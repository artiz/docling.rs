//! e2e for issue #80: the `--pages A-B` window and memory-bounded
//! referenced-image streaming.
//!
//! The PDF tests use the `no_ocr` path (text layer only), so they need pdfium
//! but no ONNX models; they skip cleanly when the pdfium library isn't around
//! (e.g. a contributor checkout before `download_dependencies.sh`).

use std::path::{Path, PathBuf};

use docling::{parse_page_range, DocumentConverter, ImageMode, SourceDocument};

/// Workspace root (this crate lives at `crates/docling`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Point pdfium resolution at the workspace-root `.pdfium/lib` (the backend's
/// default is CWD-relative, and tests run from the crate dir). Returns whether
/// the library is actually present.
fn pdfium_ready() -> bool {
    let lib = repo_root().join(".pdfium/lib");
    if lib.join("libpdfium.so").exists()
        || lib.join("libpdfium.dylib").exists()
        || lib.join("pdfium.dll").exists()
    {
        std::env::set_var("PDFIUM_DYNAMIC_LIB_PATH", &lib);
        return true;
    }
    std::env::var("PDFIUM_DYNAMIC_LIB_PATH").is_ok()
}

fn pdf_source() -> SourceDocument {
    let path = repo_root().join("tests/data/pdf/sources/2206.01062.pdf");
    SourceDocument::from_file(&path).expect("multi-page PDF fixture")
}

#[test]
fn parse_page_range_accepts_ranges_and_single_pages() {
    assert_eq!(parse_page_range("1-10"), Ok((1, 10)));
    assert_eq!(parse_page_range("7"), Ok((7, 7)));
    assert_eq!(parse_page_range(" 2 - 5 "), Ok((2, 5)));
    assert!(parse_page_range("0-3").is_err(), "pages are 1-based");
    assert!(parse_page_range("5-2").is_err(), "inverted range");
    assert!(parse_page_range("abc").is_err());
    assert!(parse_page_range("1-").is_err());
}

#[test]
fn pdf_page_window_converts_only_that_window() {
    if !pdfium_ready() {
        eprintln!("skipping: pdfium library not found");
        return;
    }
    let full = DocumentConverter::new()
        .no_ocr(true)
        .convert(pdf_source())
        .expect("full convert")
        .document;
    let windowed = DocumentConverter::new()
        .no_ocr(true)
        .page_range(2, 3)
        .convert(pdf_source())
        .expect("windowed convert")
        .document;
    assert!(!windowed.nodes.is_empty(), "window selected real pages");
    assert!(
        windowed.nodes.len() < full.nodes.len(),
        "2 of 9 pages must yield fewer nodes ({} vs {})",
        windowed.nodes.len(),
        full.nodes.len()
    );
    // A window covering the whole document is exactly the full conversion
    // (`last` clamps past the end).
    let all = DocumentConverter::new()
        .no_ocr(true)
        .page_range(1, 999)
        .convert(pdf_source())
        .expect("clamped convert")
        .document;
    assert_eq!(all.export_to_markdown(), full.export_to_markdown());
}

#[test]
fn pdf_page_window_outside_document_is_an_error() {
    if !pdfium_ready() {
        eprintln!("skipping: pdfium library not found");
        return;
    }
    let err = DocumentConverter::new()
        .no_ocr(true)
        .page_range(50, 60)
        .convert(pdf_source())
        .expect_err("window past the last page");
    assert!(
        err.to_string().contains("outside the document"),
        "unexpected error: {err}"
    );
}

/// Referenced-image streaming (#80): the stream writes image files into the
/// configured artifacts dir as chunks are emitted, and the result matches the
/// buffered `export_to_markdown_with_images` output exactly.
#[test]
fn referenced_images_stream_to_the_artifacts_dir() {
    let src = repo_root().join("tests/data/docx/sources/docx_grouped_images.docx");
    let dir = std::env::temp_dir().join(format!("docling-pages-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let artifacts_dir = dir.join("artifacts");

    let converter =
        DocumentConverter::new().artifacts_dir(artifacts_dir.to_string_lossy().into_owned());
    let stream = converter
        .convert_streaming_images(
            SourceDocument::from_file(&src).unwrap(),
            ImageMode::Referenced,
        )
        .expect("referenced mode streams now");
    let mut streamed = String::new();
    for chunk in stream {
        streamed.push_str(&chunk.expect("stream chunk"));
    }

    // Buffered reference output over the same document.
    let doc = DocumentConverter::new()
        .convert(SourceDocument::from_file(&src).unwrap())
        .unwrap()
        .document;
    let (want_md, want_artifacts) =
        doc.export_to_markdown_with_images(ImageMode::Referenced, &artifacts_dir.to_string_lossy());

    assert_eq!(streamed, want_md);
    assert!(
        !want_artifacts.is_empty(),
        "fixture must actually contain images"
    );
    for (rel, bytes) in &want_artifacts {
        let on_disk = std::fs::read(rel)
            .unwrap_or_else(|e| panic!("streamed artifact {rel} missing on disk: {e}"));
        assert_eq!(&on_disk, bytes, "artifact {rel} differs");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
