//! e2e for the warm pipeline (#669): converters lent one
//! [`WarmPipeline`] with different options in turn each get exactly the
//! document a cold converter with the same options gets — no page window,
//! OCR language, forcing or image setting carries over from the previous
//! conversion, and a change of model switches rebuilds the pipeline.
//!
//! The text-layer test needs no models; the ML sequence skips cleanly
//! without them, like `scanned.rs`.

use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument, WarmPipeline};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn source(rel: &str) -> SourceDocument {
    SourceDocument::from_file(repo_root().join("tests/data").join(rel)).expect("fixture")
}

fn ml_stack_ready() -> bool {
    let root = repo_root();
    let models_ready = ["layout_heron.onnx", "ocr_rec.onnx", "ppocr_keys_v1.txt"]
        .iter()
        .all(|m| root.join(".models").join(m).exists());
    models_ready && std::env::set_current_dir(&root).is_ok()
}

fn json(converter: DocumentConverter, rel: &str) -> String {
    converter
        .convert(source(rel))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
        .document
        .export_to_json()
}

/// One conversion: the fixture, what the step checks, and its converter.
type Step = (&'static str, &'static str, fn() -> DocumentConverter);

/// Each step on the warm handle against the same converter built cold.
fn assert_warm_matches_cold(steps: &[Step]) {
    let warm = WarmPipeline::new();
    for (rel, label, make) in steps {
        let cold = json(make(), rel);
        let warmed = json(make().warm_pipeline(&warm), rel);
        assert!(
            cold == warmed,
            "{rel} ({label}): the warm pipeline's output differs from a cold conversion"
        );
    }
}

#[test]
fn text_layer_settings_do_not_carry_over() {
    assert_warm_matches_cold(&[
        ("pdf/sources/2206.01062.pdf", "pages 2-3", || {
            DocumentConverter::new()
                .text_layer_only(true)
                .page_range(2, 3)
        }),
        // A window left on the warm pipeline would cut this one to 2-3.
        ("pdf/sources/2206.01062.pdf", "all pages", || {
            DocumentConverter::new().text_layer_only(true)
        }),
        ("pdf/sources/multi_page.pdf", "page 5", || {
            DocumentConverter::new()
                .text_layer_only(true)
                .page_range(5, 5)
        }),
        ("pdf/sources/multi_page.pdf", "all pages", || {
            DocumentConverter::new().text_layer_only(true)
        }),
    ]);
}

#[test]
fn ml_sequence_matches_cold_conversions() {
    if !ml_stack_ready() {
        eprintln!("skipping: models not found");
        return;
    }
    assert_warm_matches_cold(&[
        (
            "pdf/sources/multi_page.pdf",
            "default",
            DocumentConverter::new,
        ),
        // An OCR-language switch drops the loaded recognizer...
        ("scanned/sources/qr_bill_example.jpg", "ocr_lang ch", || {
            DocumentConverter::new().ocr_lang("ch")
        }),
        // ...and switching back must not keep the `ch` one.
        ("scanned/sources/qr_bill_example.jpg", "ocr_lang en", || {
            DocumentConverter::new().ocr_lang("en")
        }),
        (
            "pdf/sources/multi_page.pdf",
            "pages 2-3, forced OCR",
            || {
                DocumentConverter::new()
                    .page_range(2, 3)
                    .force_full_page_ocr(true)
            },
        ),
        // New model switches rebuild the pipeline, and back again.
        ("pdf/sources/multi_page.pdf", "no TableFormer", || {
            DocumentConverter::new().no_table_former(true)
        }),
        (
            "pdf/sources/multi_page.pdf",
            "default again",
            DocumentConverter::new,
        ),
    ]);
}
