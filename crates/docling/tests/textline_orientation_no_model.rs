//! #666: with the orientation switch on and the classifier missing, the
//! picture OCR warns once and keeps the oriented crop and the 90° turn —
//! the skewed line still reads, nothing fails. Its own test binary: it
//! points `DOCLING_OCR_TEXTLINE_ORI_ONNX` at nothing for the whole process.

use std::path::{Path, PathBuf};

use docling::Pipeline;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn a_missing_classifier_keeps_the_crop_and_the_turn() {
    let models = repo_root().join(".models");
    let rec = models.join("ocr_rec_v6.onnx").exists() || models.join("ocr_rec_en.onnx").exists();
    if !(rec && models.join("ocr_det.onnx").exists())
        || std::env::set_current_dir(repo_root()).is_err()
    {
        eprintln!("skipping: OCR models not found");
        return;
    }
    // SAFETY: the only test in this binary, set before any thread starts.
    unsafe {
        std::env::set_var(
            "DOCLING_OCR_TEXTLINE_ORI_ONNX",
            "/nonexistent/textline_ori.onnx",
        )
    };
    let bytes = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/textline/skewed.png"),
    )
    .expect("fixture");
    let mut p = Pipeline::new()
        .expect("pipeline")
        .no_ocr(true)
        .no_table_former(true)
        .ocr_textline_orientation(true);
    let img = p.decode_picture(&bytes).expect("decode");
    let text = p.ocr_picture(&img).expect("no failure").expect("text").text;
    let line = "Shipping address confirmed";
    assert_eq!(text, format!("{line}\n{line}"));
}
