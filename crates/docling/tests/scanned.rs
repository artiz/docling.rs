//! Scanned-PDF end-to-end conformance: the OCR pipeline's markdown for the
//! `tests/data/scanned/` fixtures must match the mirrored docling groundtruth
//! — most importantly the **rotated** scans (`/Rotate` 90/180/270), which
//! regressed invisibly for a long time because nothing consumed this
//! groundtruth.
//!
//! One test walks all four fixtures serially: each conversion runs the full ML
//! stack (layout + OCR), and four concurrent model stacks would only fight for
//! memory. It needs pdfium and the models (including the `ch` conformance OCR
//! pair the groundtruth was pinned against), so it skips cleanly on checkouts
//! without `download_dependencies.sh` — CI without models must stay green.

use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};

/// Workspace root (this crate lives at `crates/docling`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Whether pdfium and every model the scanned path loads are present (layout +
/// the `ch` OCR recognition pair). Model resolution is CWD-relative with an
/// exe-dir fallback and tests run from the crate dir, so this also moves the
/// process to the repo root — the same view the CLI has when run from there.
fn ml_stack_ready() -> bool {
    let root = repo_root();
    if !(root.join(".pdfium/lib/libpdfium.so").exists()
        || root.join(".pdfium/lib/libpdfium.dylib").exists()
        || root.join(".pdfium/lib/pdfium.dll").exists()
        || std::env::var("PDFIUM_DYNAMIC_LIB_PATH").is_ok())
    {
        return false;
    }
    let models_ready = [
        "models/layout_heron.onnx",
        "models/ocr_rec.onnx",
        "models/ppocr_keys_v1.txt",
    ]
    .iter()
    .all(|m| root.join(m).exists());
    models_ready && std::env::set_current_dir(&root).is_ok()
}

#[test]
fn scanned_fixtures_match_groundtruth() {
    if !ml_stack_ready() {
        eprintln!("skipping: pdfium/models not found");
        return;
    }
    let converter = DocumentConverter::new().ocr_lang("ch");
    for stem in [
        "ocr_test",
        "ocr_test_rotated_90",
        "ocr_test_rotated_180",
        "ocr_test_rotated_270",
    ] {
        let source = SourceDocument::from_file(format!("tests/data/scanned/sources/{stem}.pdf"))
            .expect("scanned fixture");
        let got = converter
            .convert(source)
            .expect("scanned conversion")
            .document
            .export_to_markdown();
        let want = std::fs::read_to_string(format!("tests/data/scanned/groundtruth/{stem}.md"))
            .expect("groundtruth");
        // The groundtruth files carry no trailing newline; normalize both ends.
        assert_eq!(
            got.trim_end(),
            want.trim_end(),
            "OCR markdown for {stem}.pdf diverged from tests/data/scanned/groundtruth/{stem}.md"
        );
    }
}
