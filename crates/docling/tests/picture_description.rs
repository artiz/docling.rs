//! The picture-description pass (#670): every picture to an OpenAI-compatible
//! vision endpoint, the answer as its description annotation — on DOCX,
//! PPTX, XLSX, PDF figures and an image input, with MarkItDown's prompts,
//! the picture-OCR filters, one request per distinct image, and endpoint
//! errors that warn instead of failing.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use docling::vlm::{PictureDescriptionMode, PictureDescriptionOptions, CAPTION_PROMPT, OCR_PROMPT};
use docling::{ConvertOptions, DocumentConverter, SourceDocument};
use serde_json::Value;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(rel: &str) -> PathBuf {
    let p = repo_root().join(rel);
    if p.exists() {
        p
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
    }
}

/// The layout model, which PDF and image inputs need before their pictures
/// exist; the pass itself needs no model.
fn ml_models_ready() -> bool {
    repo_root().join(".models/layout_heron.onnx").exists()
        && std::env::set_current_dir(repo_root()).is_ok()
}

/// A recorded request: the JSON body and the data-URI media type it carried.
#[derive(Clone, Debug)]
struct Seen {
    body: Value,
    mimetype: String,
}

impl Seen {
    fn prompt(&self) -> &str {
        self.body["messages"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
    }
    fn model(&self) -> &str {
        self.body["model"].as_str().unwrap()
    }
}

struct Mock {
    endpoint: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Mock {
    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

/// An OpenAI-shaped server answering every `chat/completions` with
/// `status` and a completion whose content is `answer(request index)`;
/// connections are served concurrently, one thread each.
fn mock(status: u16, answer: impl Fn(usize) -> String + Send + Sync + 'static) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let answer = Arc::new(answer);
    let counter = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { break };
            let log = Arc::clone(&log);
            let answer = Arc::clone(&answer);
            let counter = Arc::clone(&counter);
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                let (head_end, body_len) = loop {
                    let n = conn.read(&mut tmp).expect("read request");
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
                        let need: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse().ok())
                            .expect("content-length");
                        if buf.len() >= end + 4 + need {
                            break (end, need);
                        }
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
                assert!(
                    head.starts_with("POST /v1/chat/completions"),
                    "path: {head}"
                );
                let body: Value =
                    serde_json::from_slice(&buf[head_end + 4..head_end + 4 + body_len]).unwrap();
                let url = body["messages"][0]["content"][1]["image_url"]["url"]
                    .as_str()
                    .expect("image data URI");
                let mimetype = url["data:".len()..url.find(';').unwrap()].to_string();
                assert!(url.contains(";base64,"), "not a base64 data URI");
                let i = counter.fetch_add(1, Ordering::SeqCst);
                log.lock().unwrap().push(Seen { body, mimetype });
                let content = serde_json::json!({
                    "choices": [{ "message": { "role": "assistant", "content": answer(i) } }]
                })
                .to_string();
                let reason = if status == 200 { "OK" } else { "Error" };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{content}",
                    content.len()
                );
                let _ = conn.write_all(resp.as_bytes());
            });
        }
    });
    Mock {
        endpoint: format!("http://{addr}/v1"),
        seen,
    }
}

fn opts(mock: &Mock) -> PictureDescriptionOptions {
    PictureDescriptionOptions {
        endpoint: mock.endpoint.clone(),
        model: "mock-vision".into(),
        api_key: Some("sk-test".into()),
        prompt: None,
        mode: PictureDescriptionMode::Caption,
        concurrency: 1,
    }
}

fn convert(converter: DocumentConverter, rel: &str) -> docling_core::DoclingDocument {
    let source = SourceDocument::from_file(fixture(rel)).expect("fixture");
    converter.convert(source).expect("convert").document
}

fn json(doc: &docling_core::DoclingDocument) -> Value {
    serde_json::from_str(&doc.export_to_json()).expect("valid JSON")
}

/// `(meta.description.text, created_by)` per JSON picture item.
fn descriptions(json: &Value) -> Vec<(Option<String>, Option<String>)> {
    json["pictures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["meta"]["description"]["text"]
                    .as_str()
                    .map(str::to_string),
                p["meta"]["description"]["created_by"]
                    .as_str()
                    .map(str::to_string),
            )
        })
        .collect()
}

const DOCX: &str = "crates/docling/tests/data/docx/sources/picture_ocr.docx";
const PPTX: &str = "tests/data/pptx/sources/powerpoint_with_image.pptx";
const XLSX: &str = "tests/data/xlsx/sources/xlsx_08_one_cell_anchor.xlsx";
const XLSX_EMF: &str = "tests/data/xlsx/sources/xlsx_emf.xlsx";

/// DOCX: both pictures (a screenshot and a 10 px icon — the floor is lifted
/// here) get the endpoint's answer, with the model as `created_by`, in JSON
/// and in the Markdown between caption and placeholder; the request carries
/// MarkItDown's caption prompt, the model, the bearer token and a PNG data
/// URI.
#[test]
fn docx_pictures_are_described() {
    let m = mock(200, |i| format!("A caption for picture {i}."));
    let converter = DocumentConverter::new()
        .picture_description(opts(&m))
        .picture_ocr_min_side(0);
    let doc = convert(converter, DOCX);
    let seen = m.seen();
    assert_eq!(seen.len(), 2, "one request per picture: {seen:?}");
    for s in &seen {
        assert_eq!(s.prompt(), CAPTION_PROMPT);
        assert_eq!(s.model(), "mock-vision");
        assert_eq!(s.mimetype, "image/png");
        assert!(
            s.body.get("max_tokens").is_none(),
            "no max_tokens for a caption"
        );
    }
    let j = json(&doc);
    let d = descriptions(&j);
    assert_eq!(d.len(), 2);
    assert!(
        d.iter().all(|(t, by)| t
            .as_deref()
            .is_some_and(|t| t.starts_with("A caption for picture"))
            && by.as_deref() == Some("mock-vision")),
        "{d:?}"
    );
    assert_eq!(j["pictures"][0]["annotations"][0]["kind"], "description");
    let md = doc.export_to_markdown();
    let first = d[0].0.clone().unwrap();
    assert!(
        md.contains(&format!("{first}\n\n<!-- image -->")),
        "Markdown prints the description before the placeholder:\n{md}"
    );
}

/// PPTX and XLSX pictures take the same path.
#[test]
fn pptx_and_xlsx_pictures_are_described() {
    for (rel, mimetype) in [(PPTX, "image/png"), (XLSX, "image/png")] {
        let m = mock(200, |_| "What the picture shows.".into());
        let converter = DocumentConverter::new()
            .picture_description(opts(&m))
            .picture_ocr_min_side(0);
        let doc = convert(converter, rel);
        let d = descriptions(&json(&doc));
        assert!(!d.is_empty(), "{rel}: no pictures");
        assert!(
            d.iter()
                .all(|(t, _)| t.as_deref() == Some("What the picture shows.")),
            "{rel}: {d:?}"
        );
        assert_eq!(m.seen().len(), d.len(), "{rel}: one request per picture");
        assert!(m.seen().iter().all(|s| s.mimetype == mimetype), "{rel}");
        assert!(doc.export_to_markdown().contains("What the picture shows."));
    }
}

/// Whatever a backend embeds — the XLSX backend turns its EMF metafiles
/// into PNG — reaches the endpoint as one of the image types it takes, the
/// type read off the bytes.
#[test]
fn pictures_arrive_in_an_endpoint_image_type() {
    let m = mock(200, |_| "A drawing.".into());
    let converter = DocumentConverter::new()
        .picture_description(opts(&m))
        .picture_ocr_min_side(0);
    let doc = convert(converter, XLSX_EMF);
    let d = descriptions(&json(&doc));
    assert!(
        !d.is_empty() && d.iter().all(|(t, _)| t.as_deref() == Some("A drawing.")),
        "{d:?}"
    );
    for s in m.seen() {
        assert!(
            ["image/png", "image/jpeg", "image/gif", "image/webp"].contains(&s.mimetype.as_str()),
            "{}",
            s.mimetype
        );
    }
}

/// The size floor (`picture_ocr_min_side`, default 32) spares the icon one
/// request, exactly as picture OCR spares it an inference.
#[test]
fn min_side_floor_applies() {
    let m = mock(200, |_| "A caption.".into());
    let doc = convert(DocumentConverter::new().picture_description(opts(&m)), DOCX);
    let d = descriptions(&json(&doc));
    assert_eq!(d.len(), 2);
    assert_eq!(d[0].0.as_deref(), Some("A caption."));
    assert_eq!(d[1].0, None, "the 10 px icon is skipped");
    assert_eq!(m.seen().len(), 1);
}

/// `mode=ocr` sends markitdown-ocr's prompt; an explicit prompt goes
/// verbatim in either mode.
#[test]
fn ocr_mode_and_custom_prompt() {
    let m = mock(200, |_| "Text in the picture".into());
    let mut o = opts(&m);
    o.mode = PictureDescriptionMode::Ocr;
    let doc = convert(DocumentConverter::new().picture_description(o), DOCX);
    assert_eq!(m.seen()[0].prompt(), OCR_PROMPT);
    assert_eq!(
        descriptions(&json(&doc))[0].0.as_deref(),
        Some("Text in the picture")
    );

    let m = mock(200, |_| "Zwei Zeilen".into());
    let mut o = opts(&m);
    o.prompt = Some("Describe this in German.".into());
    convert(DocumentConverter::new().picture_description(o), DOCX);
    assert_eq!(m.seen()[0].prompt(), "Describe this in German.");
}

/// An empty completion is "no text", as MarkItDown reads it: the picture
/// keeps no description.
#[test]
fn blank_answers_leave_no_description() {
    let m = mock(200, |_| "  \n".into());
    let doc = convert(DocumentConverter::new().picture_description(opts(&m)), DOCX);
    assert!(descriptions(&json(&doc)).iter().all(|(t, _)| t.is_none()));
    assert_eq!(m.seen().len(), 1);
}

/// A rejected request (HTTP 400) warns and the conversion goes on without
/// the description — and the next picture is still asked about.
#[test]
fn rejected_pictures_warn_and_continue() {
    let m = mock(400, |_| "bad request".into());
    let converter = DocumentConverter::new()
        .picture_description(opts(&m))
        .picture_ocr_min_side(0);
    let doc = convert(converter, DOCX);
    assert!(descriptions(&json(&doc)).iter().all(|(t, _)| t.is_none()));
    assert_eq!(m.seen().len(), 2, "every picture is tried");
    assert!(doc.export_to_markdown().contains("<!-- image -->"));
}

/// An endpoint that cannot be reached (nothing listens) gives up after its
/// retries once, warns, and the remaining pictures are not tried — the
/// conversion still succeeds.
#[test]
fn unreachable_endpoint_warns_once_and_continues() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let o = PictureDescriptionOptions {
        endpoint: format!("http://{addr}/v1"),
        model: "mock-vision".into(),
        api_key: None,
        prompt: None,
        mode: PictureDescriptionMode::Caption,
        concurrency: 2,
    };
    let converter = DocumentConverter::new()
        .picture_description(o)
        .picture_ocr_min_side(0);
    let started = std::time::Instant::now();
    let doc = convert(converter, DOCX);
    assert!(descriptions(&json(&doc)).iter().all(|(t, _)| t.is_none()));
    // Two pictures, two workers: both start before the first gives up, but
    // nothing queues behind them — well under a third round of backoff.
    assert!(started.elapsed().as_secs() < 40, "{:?}", started.elapsed());
}

/// Several pictures in flight at once: a DOCX whose pictures are asked
/// about on three workers all come back, each with its own answer.
#[test]
fn concurrency_describes_every_picture() {
    let m = mock(200, |i| format!("caption {i}"));
    let mut o = opts(&m);
    o.concurrency = 3;
    let converter = DocumentConverter::new()
        .picture_description(o)
        .picture_ocr_min_side(0);
    let doc = convert(converter, DOCX);
    let d: Vec<String> = descriptions(&json(&doc))
        .into_iter()
        .filter_map(|(t, _)| t)
        .collect();
    assert_eq!(d.len(), 2);
    assert_ne!(d[0], d[1]);
    assert_eq!(m.seen().len(), 2);
}

/// `ConvertOptions` resolves the pass: `do_picture_description` with the
/// endpoint/model from its own fields, the `vlm_*` fields, or the
/// environment; the mode and concurrency validate; and the resolved
/// converter describes.
#[test]
fn options_resolve_and_apply() {
    let m = mock(200, |_| "Via options.".into());
    let o = ConvertOptions {
        do_picture_description: Some(true),
        picture_description_endpoint: Some(m.endpoint.clone()),
        picture_description_model: Some("mock-vision".into()),
        picture_description_mode: Some("ocr".into()),
        picture_description_concurrency: Some(2),
        ..Default::default()
    };
    let pd = o.picture_description_options().unwrap().expect("resolves");
    assert_eq!(pd.mode, PictureDescriptionMode::Ocr);
    assert_eq!(pd.concurrency, 2);
    assert_eq!(pd.prompt(), OCR_PROMPT);
    let doc = convert(o.apply(DocumentConverter::new()).unwrap(), DOCX);
    assert_eq!(
        descriptions(&json(&doc))[0].0.as_deref(),
        Some("Via options.")
    );

    // Off: nothing resolves, nothing is sent.
    let off = ConvertOptions {
        picture_description_endpoint: Some(m.endpoint.clone()),
        ..Default::default()
    };
    assert!(off.picture_description_options().unwrap().is_none());

    // The vlm_* fields stand in for the endpoint and model.
    let via_vlm = ConvertOptions {
        do_picture_description: Some(true),
        vlm_endpoint: Some(m.endpoint.clone()),
        vlm_model: Some("shared-model".into()),
        vlm_api_key: Some("k".into()),
        ..Default::default()
    };
    let pd = via_vlm
        .picture_description_options()
        .unwrap()
        .expect("resolves");
    assert_eq!(pd.model, "shared-model");
    assert_eq!(pd.api_key.as_deref(), Some("k"));

    // Bad values are rejected by name.
    let bad_mode = ConvertOptions {
        do_picture_description: Some(true),
        picture_description_mode: Some("summary".into()),
        ..Default::default()
    };
    let e = bad_mode.validate().unwrap_err().to_string();
    assert!(
        e.contains("picture_description_mode") && e.contains("caption or ocr"),
        "{e}"
    );
    let zero = ConvertOptions {
        picture_description_concurrency: Some(0),
        ..Default::default()
    };
    assert!(zero
        .validate()
        .unwrap_err()
        .to_string()
        .contains("picture_description_concurrency"));

    // Enabled without any endpoint: a named error, not a silent no-op.
    std::env::remove_var("DOCLING_RS_VLM_ENDPOINT");
    let none = ConvertOptions {
        do_picture_description: Some(true),
        ..Default::default()
    };
    let e = none.picture_description_options().unwrap_err().to_string();
    assert!(e.contains("picture_description_endpoint"), "{e}");
}

/// PDF figures — the pipeline's crops — are described like embedded
/// pictures (needs the layout model).
#[test]
fn pdf_figures_are_described() {
    if !ml_models_ready() {
        eprintln!("skipping: ML models not found");
        return;
    }
    let m = mock(200, |i| format!("Figure {i} shows a plot."));
    let doc = convert(
        DocumentConverter::new().picture_description(opts(&m)),
        "tests/data/pdf/sources/picture_classification.pdf",
    );
    let d = descriptions(&json(&doc));
    assert!(!d.is_empty(), "no PDF pictures");
    assert!(
        d.iter().all(
            |(t, by)| t.as_deref().is_some_and(|t| t.contains("shows a plot"))
                && by.as_deref() == Some("mock-vision")
        ),
        "{d:?}"
    );
    assert_eq!(m.seen().len(), d.len());
    assert!(doc.export_to_markdown().contains("shows a plot."));
}

/// An image input in caption mode is captioned as a whole — a picture
/// carrying the file, appended after what the layout model read (needs the
/// layout model); in OCR mode the page OCR already read it, so nothing is
/// added.
#[test]
fn image_inputs_get_a_caption() {
    if !ml_models_ready() {
        eprintln!("skipping: ML models not found");
        return;
    }
    let m = mock(200, |_| "A photograph of a duck.".into());
    let doc = convert(
        DocumentConverter::new().picture_description(opts(&m)),
        "tests/data/webp/sources/webp-test.webp",
    );
    assert_eq!(m.seen().len(), 1);
    assert_eq!(m.seen()[0].mimetype, "image/webp");
    let d = descriptions(&json(&doc));
    assert!(
        d.iter()
            .any(|(t, _)| t.as_deref() == Some("A photograph of a duck.")),
        "{d:?}"
    );
    let md = doc.export_to_markdown();
    assert!(
        md.trim_end()
            .ends_with("A photograph of a duck.\n\n<!-- image -->"),
        "{md}"
    );

    let m = mock(200, |_| "unexpected".into());
    let mut o = opts(&m);
    o.mode = PictureDescriptionMode::Ocr;
    let doc = convert(
        DocumentConverter::new().picture_description(o),
        "tests/data/webp/sources/webp-test.webp",
    );
    assert!(descriptions(&json(&doc)).iter().all(|(t, _)| t.is_none()));
    assert!(m.seen().is_empty());
}
