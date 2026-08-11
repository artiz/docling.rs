//! SVG input (#212), both routes.
//!
//! The pure-Rust `<text>`-extraction path is pinned against committed Markdown
//! fixtures under `tests/svg/` (its own directory — `tests/data/` is swept by
//! the generic regression harness with a *default* converter, and the default
//! SVG route is the ML rasterizer, which needs models CI doesn't have).
//! Regenerate after an intentional change with:
//!
//! ```bash
//! DOCLING_RS_REGEN=1 cargo test -p docling --test svg
//! ```
//!
//! The rasterize→image-pipeline route runs end-to-end only when the ONNX
//! models are present (skips cleanly otherwise, like `scanned.rs`).

use std::fs;
use std::path::{Path, PathBuf};

use docling::{DocumentConverter, InputFormat, SourceDocument};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/svg")
}

/// The text-extraction converter: `no_ocr` short-circuits SVG away from the
/// rasterizer on every build flavor, so this test is deterministic with or
/// without models installed.
fn text_convert(src: &Path) -> String {
    let source = SourceDocument::from_file(src).expect("svg fixture");
    DocumentConverter::new()
        .no_ocr(true)
        .convert(source)
        .expect("svg text extraction")
        .document
        .export_to_markdown()
}

#[test]
fn text_extraction_matches_fixtures() {
    let regen = std::env::var_os("DOCLING_RS_REGEN").is_some();
    let sources_dir = fixtures_dir().join("sources");
    let mut sources: Vec<PathBuf> = fs::read_dir(&sources_dir)
        .expect("tests/svg/sources missing")
        .flatten()
        .map(|e| e.path())
        .collect();
    sources.sort();
    assert!(!sources.is_empty());

    let mut failures = Vec::new();
    for src in &sources {
        let got = text_convert(src);
        let expected_path = fixtures_dir()
            .join("expected")
            .join(format!("{}.md", src.file_name().unwrap().to_string_lossy()));
        if regen {
            fs::create_dir_all(expected_path.parent().unwrap()).unwrap();
            fs::write(&expected_path, &got).unwrap();
            continue;
        }
        let expected = fs::read_to_string(&expected_path)
            .unwrap_or_else(|_| panic!("missing fixture {}", expected_path.display()));
        if got != expected {
            failures.push(format!(
                "{}: text-path Markdown drifted from the committed fixture",
                src.display()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn svg_without_text_says_why_it_needs_ml() {
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <rect width="10" height="10" fill="red"/></svg>"#;
    let err = DocumentConverter::new()
        .no_ocr(true)
        .convert(SourceDocument::from_bytes(
            "shapes",
            InputFormat::Svg,
            svg.to_vec(),
        ))
        .expect_err("no text to extract");
    assert!(
        err.to_string().contains("no text elements"),
        "unexpected error: {err}"
    );
}

/// Streaming and buffered Markdown must agree on the text path (the same
/// contract the generic regression suite enforces for every other format).
#[test]
fn streaming_matches_buffered_text_path() {
    for src in ["sources/flowchart.svg", "sources/bar_chart.svg"] {
        let path = fixtures_dir().join(src);
        let buffered = text_convert(&path);
        let source = SourceDocument::from_file(&path).unwrap();
        let mut streamed = String::new();
        for chunk in DocumentConverter::new()
            .no_ocr(true)
            .convert_streaming(source)
            .expect("stream")
        {
            streamed.push_str(&chunk.expect("chunk"));
        }
        assert_eq!(streamed, buffered, "{src}");
    }
}

/// Workspace root (this crate lives at `crates/docling`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The rasterize route needs the layout + OCR models (no pdfium — the PNG
/// decodes through the `image` crate). Model resolution is CWD-relative, so
/// this also moves the process to the repo root, like `scanned.rs`.
fn models_ready() -> bool {
    // `.models/` first, the pre-rename `models/` as fallback.
    let root = repo_root();
    ["layout_heron.onnx", "ocr_rec_en.onnx", "en_dict.txt"]
        .iter()
        .all(|m| root.join(".models").join(m).exists() || root.join("models").join(m).exists())
        && std::env::set_current_dir(&root).is_ok()
}

#[test]
fn ml_route_ocrs_rasterized_text() {
    if !models_ready() {
        eprintln!("skipping: models not found");
        return;
    }
    // Big, plain, high-contrast text — this asserts the plumbing (rasterize →
    // image pipeline → OCR), not OCR quality on hard inputs.
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="800" height="200">
        <text x="40" y="120" font-size="72" font-family="DejaVu Sans" fill="#000">HELLO DOCLING</text>
    </svg>"##;
    let doc = DocumentConverter::new()
        .convert(SourceDocument::from_bytes(
            "hello",
            InputFormat::Svg,
            svg.to_vec(),
        ))
        .expect("ml conversion")
        .document;
    let md = doc.export_to_markdown().to_lowercase();
    assert!(
        md.contains("hello") && md.contains("docling"),
        "OCR did not read the rasterized label: {md:?}"
    );
}
