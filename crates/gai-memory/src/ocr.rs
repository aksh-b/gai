//! Reading text out of images and scanned PDFs: with Tesseract (a local
//! program) or a vision model. PDF pages are rendered with `pdftoppm`
//! (poppler) when it is installed; otherwise the scans embedded in the
//! PDF (JPEG images, as scanners write them) are read directly.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use gai_core::{ContentBlock, MediaSource, Message, Role};
use gai_llm::{ChatRequest, ModelRegistry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::{MemoryError, Result};

/// ```toml
/// [knowledge.ocr]
/// backend = "auto"            # auto | tesseract | model | off
/// model = "openai/gpt-4o-mini" # a vision model, for `model` (and `auto` without Tesseract)
/// languages = "eng+deu"       # Tesseract's language packs
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct OcrSettings {
    pub backend: OcrBackend,
    /// A chat model that reads images.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The Tesseract program (default `tesseract` on the PATH).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tesseract: Option<String>,
    /// Tesseract languages, like `eng` or `eng+fra`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub languages: Option<String>,
    /// Pages read from one PDF at most.
    pub max_pages: usize,
}

impl Default for OcrSettings {
    fn default() -> Self {
        Self {
            backend: OcrBackend::Auto,
            model: None,
            tesseract: None,
            languages: None,
            max_pages: 200,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OcrBackend {
    /// Tesseract when it is installed, else the model when one is set.
    #[default]
    Auto,
    Tesseract,
    Model,
    Off,
}

/// What reads the text, once settings and the machine are known.
#[derive(Clone, Debug, PartialEq)]
pub enum Engine {
    Tesseract(String),
    Model(String),
}

impl Engine {
    /// How documents record it (`metadata.ocr`).
    pub fn label(&self) -> String {
        match self {
            Engine::Tesseract(_) => "tesseract".into(),
            Engine::Model(m) => format!("model:{m}"),
        }
    }
}

fn found(program: &str) -> bool {
    std::process::Command::new(program)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Whether `tesseract` (the default program) runs here, checked once.
fn default_tesseract() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| found("tesseract"))
}

fn pdftoppm() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| {
        std::process::Command::new("pdftoppm")
            .arg("-v")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    })
}

impl OcrSettings {
    /// The engine these settings pick on this machine, if any.
    pub fn engine(&self) -> Option<Engine> {
        let tesseract = || match &self.tesseract {
            Some(p) => found(p).then(|| p.clone()),
            None => default_tesseract().then(|| "tesseract".to_owned()),
        };
        match self.backend {
            OcrBackend::Off => None,
            OcrBackend::Tesseract => Some(Engine::Tesseract(
                self.tesseract.clone().unwrap_or_else(|| "tesseract".into()),
            )),
            OcrBackend::Model => self.model.clone().map(Engine::Model),
            OcrBackend::Auto => tesseract()
                .map(Engine::Tesseract)
                .or_else(|| self.model.clone().map(Engine::Model)),
        }
    }
}

/// The image type of `bytes`, from their first bytes.
pub fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some("image/tiff")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"BM") && bytes.len() > 26 {
        Some("image/bmp")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Image file extensions picked up when OCR is available.
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "webp"];

/// Whether a PDF's text layer is too thin to be the document (a scan).
pub fn looks_scanned(text: &str) -> bool {
    text.chars().filter(|c| c.is_alphanumeric()).count() < 40
}

/// The text of one image.
pub async fn read_image(
    engine: &Engine,
    s: &OcrSettings,
    models: &Arc<ModelRegistry>,
    bytes: &[u8],
    media_type: &str,
) -> Result<String> {
    match engine {
        Engine::Tesseract(program) => tesseract(program, s.languages.as_deref(), bytes).await,
        Engine::Model(model) => vision(models, model, bytes, media_type).await,
    }
}

/// The text of a scanned PDF, page by page.
pub async fn read_pdf(
    engine: &Engine,
    s: &OcrSettings,
    models: &Arc<ModelRegistry>,
    bytes: &[u8],
) -> Result<String> {
    let pages = page_images(bytes, s.max_pages).await?;
    if pages.is_empty() {
        return Err(MemoryError::Invalid(
            "the PDF has no text and no page images to read (install poppler's pdftoppm to render it)".into(),
        ));
    }
    let mut out = Vec::with_capacity(pages.len());
    for (i, (img, ty)) in pages.iter().enumerate() {
        let text = read_image(engine, s, models, img, ty)
            .await
            .map_err(|e| MemoryError::Invalid(format!("page {}: {e}", i + 1)))?;
        out.push(text.trim().to_owned());
    }
    Ok(out.join("\n\n"))
}

async fn tesseract(program: &str, languages: Option<&str>, image: &[u8]) -> Result<String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.arg("stdin").arg("stdout");
    if let Some(l) = languages {
        cmd.arg("-l").arg(l);
    }
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| MemoryError::Invalid(format!("could not run {program}: {e}")))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let data = image.to_vec();
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&data).await;
    });
    let out = tokio::time::timeout(Duration::from_secs(180), child.wait_with_output())
        .await
        .map_err(|_| MemoryError::Invalid(format!("{program} took over 3 minutes")))?
        .map_err(|e| MemoryError::Invalid(format!("{program}: {e}")))?;
    let _ = writer.await;
    if !out.status.success() {
        return Err(MemoryError::Invalid(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn vision(
    models: &Arc<ModelRegistry>,
    model: &str,
    image: &[u8],
    media_type: &str,
) -> Result<String> {
    let m = models.chat(model)?;
    let mut req = ChatRequest::new(vec![Message {
        role: Role::User,
        content: vec![
            ContentBlock::text("Transcribe all the text in this image."),
            ContentBlock::Image {
                source: MediaSource::Base64 {
                    media_type: media_type.to_owned(),
                    data: base64::engine::general_purpose::STANDARD.encode(image),
                },
            },
        ],
    }])
    .with_system(
        "You transcribe scanned documents. Write out every word exactly as printed, in reading order, keeping paragraphs, lists and table rows on their own lines. Output only the text, without comments; if there is no text, output nothing.",
    );
    req.temperature = Some(0.0);
    req.max_tokens = Some(4096);
    Ok(m.complete(req).await?.text())
}

/// Each page as an image: rendered by `pdftoppm`, or the page's embedded
/// scan.
async fn page_images(pdf: &[u8], max_pages: usize) -> Result<Vec<(Vec<u8>, &'static str)>> {
    if pdftoppm() {
        match render_pages(pdf, max_pages).await {
            Ok(pages) if !pages.is_empty() => return Ok(pages),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "pdftoppm failed; reading embedded images"),
        }
    }
    let pdf = pdf.to_vec();
    tokio::task::spawn_blocking(move || embedded_images(&pdf, max_pages))
        .await
        .map_err(|e| MemoryError::Invalid(format!("reading the PDF crashed: {e}")))?
}

async fn render_pages(pdf: &[u8], max_pages: usize) -> Result<Vec<(Vec<u8>, &'static str)>> {
    let dir = std::env::temp_dir().join(gai_core::new_id("ocr"));
    std::fs::create_dir_all(&dir).map_err(|e| MemoryError::Invalid(e.to_string()))?;
    let result = async {
        let input = dir.join("in.pdf");
        std::fs::write(&input, pdf).map_err(|e| MemoryError::Invalid(e.to_string()))?;
        let out = tokio::time::timeout(
            Duration::from_secs(300),
            tokio::process::Command::new("pdftoppm")
                .args(["-r", "200", "-png", "-l", &max_pages.to_string()])
                .arg(&input)
                .arg(dir.join("page"))
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| MemoryError::Invalid("pdftoppm took over 5 minutes".into()))?
        .map_err(|e| MemoryError::Invalid(format!("pdftoppm: {e}")))?;
        if !out.status.success() {
            return Err(MemoryError::Invalid(format!(
                "pdftoppm failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| MemoryError::Invalid(e.to_string()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "png"))
            .collect();
        // page-01.png, page-02.png, ... (padded to the page count).
        files.sort();
        files
            .iter()
            .map(|p| Ok((read(p)?, "image/png")))
            .collect::<Result<Vec<_>>>()
    }
    .await;
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn read(p: &Path) -> Result<Vec<u8>> {
    std::fs::read(p).map_err(|e| MemoryError::Invalid(format!("{}: {e}", p.display())))
}

/// The largest JPEG image on each page, as scanners embed them.
fn embedded_images(pdf: &[u8], max_pages: usize) -> Result<Vec<(Vec<u8>, &'static str)>> {
    use lopdf::{Document, Object};
    let doc = Document::load_mem(pdf)
        .map_err(|e| MemoryError::Invalid(format!("could not read the PDF: {e}")))?;
    let mut out = Vec::new();
    for (_, page_id) in doc.get_pages().into_iter().take(max_pages) {
        let mut best: Option<Vec<u8>> = None;
        let Ok((resources, ids)) = doc.get_page_resources(page_id) else {
            continue;
        };
        let mut dicts: Vec<&lopdf::Dictionary> = resources.into_iter().collect();
        let owned: Vec<lopdf::Dictionary> = ids
            .iter()
            .filter_map(|id| doc.get_dictionary(*id).ok().cloned())
            .collect();
        dicts.extend(owned.iter());
        for res in dicts {
            let Ok(xobjects) = res
                .get(b"XObject")
                .and_then(|o| doc.dereference(o))
                .and_then(|(_, o)| o.as_dict())
            else {
                continue;
            };
            for (_, obj) in xobjects.iter() {
                let Ok((_, Object::Stream(s))) = doc.dereference(obj) else {
                    continue;
                };
                let is_image = s
                    .dict
                    .get(b"Subtype")
                    .and_then(Object::as_name)
                    .is_ok_and(|n| n == b"Image");
                let jpeg = match s.dict.get(b"Filter") {
                    Ok(Object::Name(n)) => n == b"DCTDecode",
                    Ok(Object::Array(a)) => {
                        a.last().and_then(|f| f.as_name().ok()) == Some(b"DCTDecode".as_slice())
                            && a.len() == 1
                    }
                    _ => false,
                };
                if is_image && jpeg && best.as_ref().is_none_or(|b| s.content.len() > b.len()) {
                    best = Some(s.content.clone());
                }
            }
        }
        if let Some(img) = best {
            out.push((img, "image/jpeg"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_types_and_scans() {
        assert_eq!(image_type(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(image_type(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(image_type(b"II*\0...."), Some("image/tiff"));
        assert_eq!(image_type(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_type(b"%PDF-1.4"), None);
        assert!(looks_scanned("  \n\x0c  1 "));
        assert!(!looks_scanned(
            &"This agreement is made between the parties. ".repeat(3)
        ));
        let off = OcrSettings {
            backend: OcrBackend::Off,
            ..Default::default()
        };
        assert_eq!(off.engine(), None);
        let model = OcrSettings {
            backend: OcrBackend::Model,
            model: Some("x/vision".into()),
            ..Default::default()
        };
        assert_eq!(model.engine(), Some(Engine::Model("x/vision".into())));
        assert_eq!(model.engine().unwrap().label(), "model:x/vision");
    }
}
