//! Images and scanned PDFs read by OCR on ingest: a vision model
//! (scripted) and, where it is installed, Tesseract.

use std::sync::Arc;

use async_trait::async_trait;
use gai_core::{ContentBlock, MediaSource};
use gai_llm::{
    LlmConfig, LlmError, MockModel, ModelInfo, ModelRegistry, Provider, SharedChatModel,
    text_response,
};
use gai_memory::ocr::{OcrBackend, OcrSettings};
use gai_memory::{Knowledge, KnowledgeSettings, NewCollection, SearchMode};
use gai_store::{SharedStore, SqliteStore};

struct Scripted(Arc<MockModel>);

#[async_trait]
impl Provider for Scripted {
    fn name(&self) -> &str {
        "script"
    }
    fn chat(&self, _info: ModelInfo) -> Result<SharedChatModel, LlmError> {
        Ok(self.0.clone())
    }
}

async fn knowledge(ocr: OcrSettings, script: Option<Arc<MockModel>>) -> Knowledge {
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let mut b = ModelRegistry::builder().config(LlmConfig::default());
    if let Some(m) = script {
        b = b.provider("script", Arc::new(Scripted(m)));
    }
    let k = Knowledge::new(
        store,
        Arc::new(b.build().unwrap()),
        KnowledgeSettings {
            ocr,
            ..Default::default()
        },
    );
    k.create_collection(NewCollection {
        name: "contracts".into(),
        ..Default::default()
    })
    .await
    .unwrap();
    k
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR not a real picture";
/// What scanners write: JPEG data, here only its first bytes.
const JPEG: &[u8] = &[
    0xff, 0xd8, 0xff, 0xe0, 0, 0x10, b'J', b'F', b'I', b'F', 0, 1, 2, 3, 4, 5,
];

/// A PDF with no text: one page showing one JPEG image.
fn scanned_pdf(jpeg: &[u8]) -> Vec<u8> {
    let content = "q 612 0 0 792 0 0 cm /Im1 Do Q";
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /XObject << /Im1 5 0 R >> >> >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes(),
    ];
    let mut img = format!(
        "<< /Type /XObject /Subtype /Image /Width 10 /Height 10 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
        jpeg.len()
    )
    .into_bytes();
    img.extend_from_slice(jpeg);
    img.extend_from_slice(b"\nendstream");
    objects.push(img);
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n", i + 1).bytes());
        out.extend_from_slice(o);
        out.extend(b"\nendobj\n");
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

fn sent_image(m: &MockModel) -> Option<(String, String)> {
    m.requests().lock().unwrap().iter().find_map(|r| {
        r.messages
            .iter()
            .flat_map(|msg| &msg.content)
            .find_map(|b| match b {
                ContentBlock::Image {
                    source: MediaSource::Base64 { media_type, data },
                } => Some((media_type.clone(), data.clone())),
                _ => None,
            })
    })
}

#[tokio::test]
async fn a_vision_model_reads_images_and_scans() {
    let script = Arc::new(MockModel::scripted(vec![
        text_response("MASTER SERVICES AGREEMENT\nTerm: 24 months. Governing law: Delaware."),
        text_response("LEASE\nMonthly rent is 4,200 dollars, due on the first."),
    ]));
    let k = knowledge(
        OcrSettings {
            backend: OcrBackend::Model,
            model: Some("script/vision".into()),
            ..Default::default()
        },
        Some(script.clone()),
    )
    .await;
    assert!(k.ocr_available());

    let msa = k
        .ingest_bytes(
            "contracts",
            "msa-scan.png",
            None,
            PNG.to_vec(),
            None,
            serde_json::json!({}),
        )
        .await
        .unwrap();
    assert_eq!(msa.document.metadata["format"], "image");
    assert_eq!(msa.document.metadata["ocr"], "model:script/vision");
    assert_eq!(msa.document.title.as_deref(), Some("msa-scan"));
    assert_eq!(sent_image(&script).unwrap().0, "image/png");

    let lease = k
        .ingest_bytes(
            "contracts",
            "lease.pdf",
            None,
            scanned_pdf(JPEG),
            None,
            serde_json::json!({}),
        )
        .await
        .unwrap();
    assert_eq!(lease.document.metadata["format"], "pdf");
    assert_eq!(lease.document.metadata["ocr"], "model:script/vision");
    // The page's scan went to the model as it was embedded (unless
    // pdftoppm rendered the page instead).
    let sent: Vec<String> = script
        .requests()
        .lock()
        .unwrap()
        .iter()
        .flat_map(|r| r.messages.iter().flat_map(|m| m.content.clone()))
        .filter_map(|b| match b {
            ContentBlock::Image {
                source: MediaSource::Base64 { media_type, .. },
            } => Some(media_type),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 2);
    assert!(
        sent[1] == "image/jpeg" || sent[1] == "image/png",
        "{sent:?}"
    );

    let hits = k
        .search(&["contracts".into()], "governing law", 5, SearchMode::Text)
        .await
        .unwrap();
    assert_eq!(hits[0].source, "msa-scan.png");
    let hits = k
        .search(&["contracts".into()], "rent", 5, SearchMode::Text)
        .await
        .unwrap();
    assert_eq!(hits[0].source, "lease.pdf");
}

#[tokio::test]
async fn without_ocr_images_are_refused_and_skipped() {
    let k = knowledge(
        OcrSettings {
            backend: OcrBackend::Off,
            ..Default::default()
        },
        None,
    )
    .await;
    assert!(!k.ocr_available());
    let err = k
        .ingest_bytes(
            "contracts",
            "scan.png",
            None,
            PNG.to_vec(),
            None,
            serde_json::json!({}),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("knowledge.ocr"), "{err}");
    let err = k
        .ingest_bytes(
            "contracts",
            "scan.pdf",
            None,
            scanned_pdf(JPEG),
            None,
            serde_json::json!({}),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("knowledge.ocr"), "{err}");
    // Folders skip pictures then.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.png"), PNG).unwrap();
    std::fs::write(dir.path().join("b.md"), "# Notes\n\nPlain text.").unwrap();
    let done = k.ingest_path("contracts", dir.path()).await.unwrap();
    assert_eq!(done.len(), 1);
}

/// A PDF showing `text` in Helvetica.
fn text_pdf(text: &str) -> Vec<u8> {
    let stream = format!("BT /F1 28 Tf 72 700 Td ({text}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
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
    out.extend(format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").bytes());
    out
}

#[tokio::test]
async fn tesseract_reads_a_rendered_page() {
    let have = |p: &str, arg: &str| std::process::Command::new(p).arg(arg).output().is_ok();
    if !have("tesseract", "--version") || !have("pdftoppm", "-v") {
        eprintln!("skipped: install tesseract-ocr and poppler-utils to run this test");
        return;
    }
    // Render a text PDF to a picture, as a scanner would have made it.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("in.pdf"),
        text_pdf("Invoice total 1250 EUR"),
    )
    .unwrap();
    let out = std::process::Command::new("pdftoppm")
        .args(["-r", "150", "-png", "-singlefile"])
        .arg(dir.path().join("in.pdf"))
        .arg(dir.path().join("page"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let png = std::fs::read(dir.path().join("page.png")).unwrap();

    let k = knowledge(
        OcrSettings {
            backend: OcrBackend::Tesseract,
            ..Default::default()
        },
        None,
    )
    .await;
    let doc = k
        .ingest_bytes(
            "contracts",
            "invoice.png",
            None,
            png,
            None,
            serde_json::json!({}),
        )
        .await
        .unwrap();
    assert_eq!(doc.document.metadata["ocr"], "tesseract");
    let hits = k
        .search(&["contracts".into()], "invoice total", 5, SearchMode::Text)
        .await
        .unwrap();
    assert_eq!(
        hits.len(),
        1,
        "OCR read: {:?}",
        k.store().document_chunks(&doc.document.id).await.unwrap()
    );
}
