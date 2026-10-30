//! A sideways page drawn as vector outlines beside a thin native text layer
//! (#936): the orientation probe used to run only on pages with no text cell
//! at all, so the one upright `Page 1` footer left the table unread (6 words
//! against 242 with `force_ocr`). Needs the models; skips without them so CI
//! without models stays green.

use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn ml_stack_ready() -> bool {
    let root = repo_root();
    let models_ready = ["layout_heron.onnx", "ocr_rec.onnx", "ppocr_keys_v1.txt"]
        .iter()
        .all(|m| root.join(".models").join(m).exists());
    models_ready && std::env::set_current_dir(&root).is_ok()
}

#[test]
fn sideways_outlined_table_beside_a_footer_is_read() {
    if !ml_stack_ready() {
        eprintln!("skipping: models not found");
        return;
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sideways_outlines/rotated-table-page.pdf");
    let md = DocumentConverter::new()
        .ocr_lang("ch")
        .convert(SourceDocument::from_file(path).expect("fixture"))
        .expect("conversion")
        .document
        .export_to_markdown();
    assert!(
        md.matches("inspection reports").count() >= 3,
        "the sideways table was not read: {md:?}"
    );
    assert!(md.contains("spreadsheet"), "{md:?}");
}
