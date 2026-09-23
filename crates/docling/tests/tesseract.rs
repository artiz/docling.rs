//! #460: the Tesseract OCR engine end to end — the system `tesseract` binary
//! reading the scanned fixtures through the same layout-region pipeline as
//! PP-OCR, selected with `ocr_engine("tesseract")`. Needs pdfium, the layout
//! model and a `tesseract` with the `eng` traineddata on `PATH`, so it skips
//! cleanly on checkouts without them — CI without models must stay green.
//! Text assertions are deliberately loose (Tesseract versions differ by a
//! character here and there); the rotation test compares two runs of the
//! same engine, which is what OSD-driven un-rotation must make identical.

use std::path::{Path, PathBuf};
use std::process::Command;

use docling::{DocumentConverter, SourceDocument};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Tesseract's installed traineddata stems, or `None` when the binary is not
/// runnable.
fn tesseract_langs() -> Option<Vec<String>> {
    let out = Command::new("tesseract")
        .arg("--list-langs")
        .output()
        .ok()?;
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .skip(1)
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
    )
}

/// pdfium + the layout model + `tesseract` with `eng`; moves the process to
/// the repo root (model resolution is CWD-relative).
fn ready(need: &[&str]) -> bool {
    let root = repo_root();
    if !(root.join(".pdfium/lib/libpdfium.so").exists()
        || root.join(".pdfium/lib/libpdfium.dylib").exists()
        || root.join(".pdfium/lib/pdfium.dll").exists()
        || std::env::var("PDFIUM_DYNAMIC_LIB_PATH").is_ok())
    {
        return false;
    }
    if !root.join(".models/layout_heron.onnx").exists() {
        return false;
    }
    let Some(langs) = tesseract_langs() else {
        return false;
    };
    need.iter().all(|n| langs.iter().any(|l| l == n)) && std::env::set_current_dir(&root).is_ok()
}

fn convert(stem: &str) -> String {
    let source = SourceDocument::from_file(format!("tests/data/scanned/sources/{stem}.pdf"))
        .expect("scanned fixture");
    DocumentConverter::new()
        .ocr_engine("tesseract")
        .ocr_lang("eng")
        .convert(source)
        .expect("tesseract conversion")
        .document
        .export_to_markdown()
}

/// A scanned page reads out through Tesseract: the fixture's one sentence,
/// as a paragraph, with its words intact.
#[test]
fn tesseract_engine_reads_a_scanned_page() {
    if !ready(&["eng"]) {
        eprintln!("skipping: pdfium, the layout model or tesseract (eng) is not present");
        return;
    }
    let md = convert("ocr_test");
    for word in ["Docling", "PDF", "JSON", "Markdown", "package"] {
        assert!(
            md.contains(word),
            "{word:?} missing from Tesseract output:\n{md}"
        );
    }
    assert_eq!(
        md.trim().lines().count(),
        1,
        "one paragraph expected:\n{md}"
    );
}

/// #225 under Tesseract: a raster physically rotated inside the page
/// (`/Rotate 0`) is un-rotated from the engine's own OSD, so the four
/// orientations of the same scan produce the same text.
#[test]
fn tesseract_osd_normalizes_raster_rotation() {
    if !ready(&["eng", "osd"]) {
        eprintln!("skipping: pdfium, the layout model or tesseract (eng + osd) is not present");
        return;
    }
    let upright = convert("ocr_test_raster");
    assert!(upright.contains("Docling"), "upright run:\n{upright}");
    for stem in [
        "ocr_test_raster_rot_90",
        "ocr_test_raster_rot_180",
        "ocr_test_raster_rot_270",
    ] {
        assert_eq!(
            convert(stem).trim_end(),
            upright.trim_end(),
            "OSD orientation failed to normalize {stem}.pdf"
        );
    }
}

/// An `ocr_lang` Tesseract cannot serve degrades like a missing model (#244):
/// the conversion succeeds with the page's text empty, never errors.
#[test]
fn tesseract_missing_language_degrades() {
    if !ready(&["eng"]) {
        eprintln!("skipping: pdfium, the layout model or tesseract (eng) is not present");
        return;
    }
    let source = SourceDocument::from_file("tests/data/scanned/sources/ocr_test.pdf")
        .expect("scanned fixture");
    let md = DocumentConverter::new()
        .ocr_engine("tesseract")
        .ocr_lang("xyz_nonexistent")
        .convert(source)
        .expect("a missing traineddata degrades instead of erroring")
        .document
        .export_to_markdown();
    assert!(
        !md.contains("Docling"),
        "no text expected without the language:\n{md}"
    );
}
