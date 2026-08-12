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

/// The layout + OCR models the force-OCR test needs, beyond pdfium. Model
/// resolution is CWD-relative (tests run from the crate dir) so, when the
/// repo-root copies exist, point the env overrides at them; skips cleanly on
/// a model-free checkout, same as the pdfium gate.
fn ocr_models_ready() -> bool {
    let m = repo_root().join(".models");
    let layout = ["layout_heron_int8.onnx", "layout_heron.onnx"]
        .iter()
        .map(|f| m.join(f))
        .find(|p| p.exists());
    let rec = ["ocr_rec_en.onnx", "ocr_rec.onnx"]
        .iter()
        .map(|f| m.join(f))
        .find(|p| p.exists());
    let dict = ["en_dict.txt", "ppocr_keys_v1.txt"]
        .iter()
        .map(|f| m.join(f))
        .find(|p| p.exists());
    match (layout, rec, dict) {
        (Some(l), Some(r), Some(d)) => {
            std::env::set_var("DOCLING_LAYOUT_ONNX", l);
            std::env::set_var("DOCLING_OCR_REC_ONNX", r);
            std::env::set_var("DOCLING_OCR_DICT", d);
            true
        }
        _ => false,
    }
}

/// `force_full_page_ocr` (docling's option of the same name) must actually
/// discard the text layer: converting a digital PDF page with it produces
/// OCR-recognized text, not the embedded cells. The fixture's text layer
/// spells "JavaScript Code Example" — OCR of the rendered page reads the same
/// heading, so the words prove the page converted *some* way; the differing
/// glyph geometry (OCR boxes never byte-match the text layer's) proves it was
/// not the text-layer path: the two outputs must differ.
#[test]
fn force_full_page_ocr_discards_the_text_layer() {
    if !pdfium_ready() || !ocr_models_ready() {
        eprintln!("skipping: pdfium or the OCR models are not present");
        return;
    }
    let src = || {
        SourceDocument::from_file(repo_root().join("tests/data/pdf/sources/code_and_formula.pdf"))
            .expect("fixture")
    };
    let normal = DocumentConverter::new()
        .page_range(1, 1)
        .convert(src())
        .expect("normal convert")
        .document
        .export_to_markdown();
    let forced = DocumentConverter::new()
        .page_range(1, 1)
        .force_full_page_ocr(true)
        .convert(src())
        .expect("forced convert")
        .document
        .export_to_markdown();
    assert!(
        forced.contains("JavaScript"),
        "OCR should still read the page's heading: {forced:?}"
    );
    assert_ne!(
        normal, forced,
        "forced output must come from OCR, not the embedded text layer"
    );
}

/// #183: the PDF pipeline attaches a per-page confidence report to the
/// converted document. On a digital page the layout model scores real
/// clusters (well above the 0.3 label floor), the text layer parses cleanly
/// (parse_score ≈ 1.0), and nothing came from OCR — so ocr_score stays unset
/// and table_score is always unset (docling never assigns it either).
#[test]
fn pdf_pipeline_reports_confidence() {
    if !pdfium_ready() || !ocr_models_ready() {
        eprintln!("skipping: pdfium or the OCR models are not present");
        return;
    }
    let src =
        SourceDocument::from_file(repo_root().join("tests/data/pdf/sources/2305.03393v1-pg9.pdf"))
            .expect("single-page fixture");
    let doc = DocumentConverter::new()
        .convert(src)
        .expect("convert")
        .document;
    let report = doc.confidence.as_ref().expect("pipeline sets confidence");
    assert_eq!(report.pages.len(), 1, "one page, one entry");
    let page = report.pages.get(&1).expect("keyed by 1-based page number");
    let layout = page.layout_score.expect("layout ran");
    assert!((0.3..=1.0).contains(&layout), "layout_score {layout}");
    let parse = page.parse_score.expect("text layer present");
    assert!(parse > 0.9, "clean digital text layer, got {parse}");
    assert_eq!(page.ocr_score, None, "digital page: nothing OCR'd");
    assert_eq!(page.table_score, None, "never assigned (docling parity)");
    // Aggregates exist and grade sanely.
    let mean = report.mean_score().expect("scores present");
    assert!((0.3..=1.0).contains(&mean), "mean_score {mean}");
    assert_ne!(
        report.mean_grade().as_str(),
        "unspecified",
        "scored conversion must grade"
    );
    // The report is a response-level extra, not part of the document schema:
    // the JSON export must stay byte-identical to a confidence-free document.
    assert!(
        !doc.export_to_json().contains("confidence"),
        "confidence must not leak into the docling-JSON export"
    );
}

/// #183 on the no-OCR text-layer path: parse quality still scores (the cells
/// are the same extraction the pipeline sees), layout is the orphan-rescue
/// set (cell confidence 1.0), and OCR never ran. Needs pdfium only.
#[test]
fn no_ocr_conversion_reports_parse_confidence() {
    if !pdfium_ready() {
        eprintln!("skipping: pdfium library not found");
        return;
    }
    let doc = DocumentConverter::new()
        .no_ocr(true)
        .page_range(1, 2)
        .convert(pdf_source())
        .expect("convert")
        .document;
    let report = doc.confidence.as_ref().expect("pipeline sets confidence");
    assert_eq!(report.pages.len(), 2, "one entry per converted page");
    assert!(report.pages.contains_key(&1) && report.pages.contains_key(&2));
    let parse = report.parse_score().expect("text layer parsed");
    assert!(parse > 0.5, "clean text layer, got {parse}");
    assert_eq!(report.ocr_score(), None, "no OCR on the no_ocr path");
}

/// TableFormer models present too (encoder/decoder/bbox next to the layout
/// model) — the cell-geometry test needs the ML table path, not the
/// geometric fallback.
fn tableformer_ready() -> bool {
    let tf = repo_root().join(".models/tableformer");
    let dec = ["decoder_kv.onnx", "decoder_int8.onnx", "decoder.onnx"]
        .iter()
        .map(|f| tf.join(f))
        .find(|p| p.exists());
    match dec {
        Some(d) if tf.join("encoder.onnx").exists() && tf.join("bbox.onnx").exists() => {
            std::env::set_var("DOCLING_TABLEFORMER_ENCODER", tf.join("encoder.onnx"));
            std::env::set_var("DOCLING_TABLEFORMER_DECODER", d);
            std::env::set_var("DOCLING_TABLEFORMER_BBOX", tf.join("bbox.onnx"));
            true
        }
        _ => false,
    }
}

/// #238: the ML table path records per-cell page-point boxes on the public
/// `Table`, and the bbox-driven repair API round-trips: locate a cell by its
/// own recorded box, replace the text, see it in the re-export.
#[test]
fn table_cells_carry_boxes_and_support_bbox_repair() {
    if !pdfium_ready() || !ocr_models_ready() || !tableformer_ready() {
        eprintln!("skipping: pdfium or the ML models are not present");
        return;
    }
    let src =
        SourceDocument::from_file(repo_root().join("tests/data/pdf/sources/2305.03393v1-pg9.pdf"))
            .expect("table fixture");
    let mut document = DocumentConverter::new()
        .convert(src)
        .expect("convert")
        .document;

    let table = document
        .tables_mut()
        .find(|t| t.cell_boxes.is_some())
        .expect("the fixture's table goes through TableFormer and carries geometry");
    let (r, c, bbox) = (0..table.rows.len())
        .flat_map(|r| (0..table.rows[r].len()).map(move |c| (r, c)))
        .find_map(|(r, c)| {
            (!table.cell_text(r, c).unwrap_or_default().is_empty())
                .then(|| table.cell_bbox(r, c).map(|b| (r, c, b)))
                .flatten()
        })
        .expect("a non-empty cell with geometry");
    // Boxes are page points, top-left origin: a sane, non-degenerate rect.
    assert!(
        bbox[2] > bbox[0] && bbox[3] > bbox[1],
        "degenerate {bbox:?}"
    );
    assert_eq!(table.find_cell_by_bbox(bbox), Some((r, c)));
    assert_eq!(
        table.update_cell_by_bbox(bbox, "REPAIRED-238"),
        Some((r, c))
    );
    assert!(
        document.export_to_markdown().contains("REPAIRED-238"),
        "repair flows into the export"
    );
}
