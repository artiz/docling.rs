//! Text drawn as vector outlines on a page that also has a text layer (#931):
//! the layout model finds the paragraph, the text layer has nothing there,
//! and the region must be OCR'd as docling's default `pdf_aware_layout_regions`
//! does — a single native footer used to switch the whole page's OCR off.
//! Needs the models; skips without them so CI without models stays green.

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

fn markdown(converter: DocumentConverter) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vector_text/vector-text-page.pdf");
    converter
        .convert(SourceDocument::from_file(path).expect("fixture"))
        .expect("conversion")
        .document
        .export_to_markdown()
}

#[test]
fn outlined_text_beside_a_text_layer_is_ocrd() {
    if !ml_stack_ready() {
        eprintln!("skipping: models not found");
        return;
    }
    let md = markdown(DocumentConverter::new().ocr_lang("ch"));
    assert!(
        md.matches("pump station").count() >= 5,
        "the outlined paragraphs were not OCR'd: {md:?}"
    );
    assert!(md.contains("quarterly maintenance report"), "{md:?}");

    // `no_ocr` keeps the old reading: the text layer alone, which here is
    // only the footer (furniture, not in the Markdown).
    let md = markdown(DocumentConverter::new().no_ocr(true));
    assert!(!md.contains("pump station"), "{md:?}");
}
