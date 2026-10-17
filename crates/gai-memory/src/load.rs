//! Turning files into plain text: text and Markdown as they are, HTML with
//! tags stripped, PDF through text extraction.

use std::path::Path;

use crate::MemoryError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Text,
    Markdown,
    Html,
    Pdf,
}

#[derive(Clone, Debug)]
pub struct Loaded {
    pub title: Option<String>,
    pub text: String,
    pub format: Format,
}

/// Extensions read when ingesting a directory. Anything else is skipped.
const TEXT_EXTENSIONS: &[&str] = &[
    "txt",
    "text",
    "md",
    "markdown",
    "mdx",
    "rst",
    "adoc",
    "org",
    "tex",
    "csv",
    "tsv",
    "json",
    "jsonl",
    "yaml",
    "yml",
    "toml",
    "xml",
    "ini",
    "log",
    "rs",
    "py",
    "js",
    "jsx",
    "ts",
    "tsx",
    "go",
    "java",
    "kt",
    "c",
    "h",
    "cc",
    "cpp",
    "hpp",
    "cs",
    "rb",
    "php",
    "swift",
    "scala",
    "sh",
    "bash",
    "ps1",
    "sql",
    "css",
    "scss",
    "vue",
    "svelte",
    "lua",
    "r",
    "dart",
    "ex",
    "exs",
    "hs",
    "ml",
    "zig",
    "proto",
    "graphql",
    "dockerfile",
];

/// Whether a directory walk should pick this file up.
pub fn is_supported(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.starts_with('.') {
        return false;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(ext.as_str(), "pdf" | "html" | "htm" | "xhtml")
        || TEXT_EXTENSIONS.contains(&ext.as_str())
        || matches!(
            name.as_str(),
            "readme" | "license" | "dockerfile" | "makefile"
        )
}

pub fn detect(name: &str, content_type: Option<&str>, bytes: &[u8]) -> Format {
    let ct = content_type.unwrap_or_default().to_ascii_lowercase();
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if bytes.starts_with(b"%PDF") || ct.contains("pdf") || ext == "pdf" {
        Format::Pdf
    } else if ct.contains("html") || matches!(ext.as_str(), "html" | "htm" | "xhtml") {
        Format::Html
    } else if ct.contains("markdown") || matches!(ext.as_str(), "md" | "markdown" | "mdx") {
        Format::Markdown
    } else {
        Format::Text
    }
}

/// Read a file into text, choosing the loader by extension and content.
pub fn load_path(path: &Path) -> Result<Loaded, MemoryError> {
    let bytes = std::fs::read(path)
        .map_err(|e| MemoryError::Invalid(format!("{}: {e}", path.display())))?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    load_bytes(name, None, &bytes)
}

/// Turn uploaded bytes into text. `name` is a file name (for the format and
/// a fallback title); `content_type` a MIME type, when known.
pub fn load_bytes(
    name: &str,
    content_type: Option<&str>,
    bytes: &[u8],
) -> Result<Loaded, MemoryError> {
    let format = detect(name, content_type, bytes);
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let (title, text) = match format {
        Format::Pdf => (stem, pdf_text(bytes)?),
        _ => {
            if bytes.iter().take(8000).any(|b| *b == 0) {
                return Err(MemoryError::Invalid(format!(
                    "{name} looks like a binary file; supported: text, Markdown, HTML and PDF"
                )));
            }
            let raw = String::from_utf8_lossy(bytes);
            let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
            match format {
                Format::Html => (
                    html_title(raw).or(stem),
                    gai_tools::builtin::html_to_text(raw),
                ),
                Format::Markdown => (markdown_title(raw).or(stem), raw.to_owned()),
                _ => (stem, raw.to_owned()),
            }
        }
    };
    let text = text.replace("\r\n", "\n");
    if text.trim().is_empty() {
        return Err(MemoryError::Invalid(format!("{name} has no text")));
    }
    Ok(Loaded {
        title,
        text,
        format,
    })
}

fn markdown_title(text: &str) -> Option<String> {
    let body = match text.strip_prefix("---\n") {
        // Skip YAML front matter, taking its title if it has one.
        Some(rest) => {
            let end = rest.find("\n---")?;
            if let Some(t) = rest[..end]
                .lines()
                .find_map(|l| l.strip_prefix("title:"))
                .map(|t| t.trim().trim_matches(['"', '\'']).to_owned())
                .filter(|t| !t.is_empty())
            {
                return Some(t);
            }
            &rest[end + 4..]
        }
        None => text,
    };
    body.lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
}

fn html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = start + lower[start..].find('>')? + 1;
    let close = open_end + lower[open_end..].find("</title")?;
    let t = gai_tools::builtin::html_to_text(&html[open_end..close]);
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

#[cfg(feature = "pdf")]
fn pdf_text(bytes: &[u8]) -> Result<String, MemoryError> {
    // The extractor panics on some malformed files; treat that as an error.
    let bytes = bytes.to_vec();
    match std::panic::catch_unwind(move || pdf_extract::extract_text_from_mem(&bytes)) {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(MemoryError::Invalid(format!("could not read the PDF: {e}"))),
        Err(_) => Err(MemoryError::Invalid(
            "could not read the PDF: the file is malformed or uses an unsupported feature".into(),
        )),
    }
}

#[cfg(not(feature = "pdf"))]
fn pdf_text(_bytes: &[u8]) -> Result<String, MemoryError> {
    Err(MemoryError::Invalid(
        "this build of gai cannot read PDFs (built without the `pdf` feature)".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_and_titles() {
        let md = load_bytes(
            "guide.md",
            None,
            b"---\ntitle: \"The Guide\"\n---\n# Heading\nBody",
        )
        .unwrap();
        assert_eq!(
            (md.format, md.title.as_deref()),
            (Format::Markdown, Some("The Guide"))
        );
        let md = load_bytes("guide.md", None, b"Intro\n# Heading\nBody").unwrap();
        assert_eq!(md.title.as_deref(), Some("Heading"));
        let html = load_bytes(
            "page",
            Some("text/html; charset=utf-8"),
            b"<html><head><title>Hi &amp; bye</title></head><body><p>Para</p><script>x()</script></body></html>",
        )
        .unwrap();
        assert_eq!(html.format, Format::Html);
        assert_eq!(html.title.as_deref(), Some("Hi & bye"));
        assert!(html.text.contains("Para") && !html.text.contains("x()"));
        let txt = load_bytes("notes.txt", None, b"\xef\xbb\xbfline one\r\nline two").unwrap();
        assert_eq!(
            (txt.title.as_deref(), txt.text.as_str()),
            (Some("notes"), "line one\nline two")
        );
        assert!(load_bytes("a.bin", None, b"ab\0cd").is_err());
        assert!(load_bytes("empty.txt", None, b"  \n").is_err());
    }

    /// A one-page PDF showing `text` in Helvetica, with a correct xref.
    fn tiny_pdf(text: &str) -> Vec<u8> {
        let stream = format!("BT /F1 12 Tf 72 712 Td ({text}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_owned(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for off in offsets {
            out.extend(format!("{off:010} 00000 n \n").bytes());
        }
        out.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .bytes(),
        );
        out
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn reads_pdf_text() {
        let l = load_bytes("report.pdf", None, &tiny_pdf("Quarterly revenue grew")).unwrap();
        assert_eq!(l.format, Format::Pdf);
        assert_eq!(l.title.as_deref(), Some("report"));
        assert!(l.text.contains("Quarterly revenue grew"), "{:?}", l.text);
    }

    #[test]
    fn broken_pdf_is_an_error_not_a_panic() {
        assert!(load_bytes("x.pdf", None, b"%PDF-1.4 garbage").is_err());
    }

    #[test]
    fn walk_filter() {
        assert!(is_supported(Path::new("docs/a.MD")));
        assert!(is_supported(Path::new("x/report.pdf")));
        assert!(is_supported(Path::new("README")));
        assert!(!is_supported(Path::new("img.png")));
        assert!(!is_supported(Path::new(".env")));
    }
}
