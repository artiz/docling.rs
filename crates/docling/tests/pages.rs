//! e2e for issue #80: the `--pages A-B` window and memory-bounded
//! referenced-image streaming.
//!
//! The PDF tests use the `text_layer_only` path (text layer only), so they need no
//! native library and no ONNX models; the OCR/TableFormer ones skip cleanly
//! when the models aren't around (e.g. a contributor checkout before
//! `download_dependencies.sh`).

use std::path::{Path, PathBuf};

use docling::{parse_page_range, DocumentConverter, ImageMode, SourceDocument, TextLayerOnly};

/// Workspace root (this crate lives at `crates/docling`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
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
    let full = DocumentConverter::new()
        .text_layer_only(true)
        .convert(pdf_source())
        .expect("full convert")
        .document;
    let windowed = DocumentConverter::new()
        .text_layer_only(true)
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
        .text_layer_only(true)
        .page_range(1, 999)
        .convert(pdf_source())
        .expect("clamped convert")
        .document;
    assert_eq!(all.export_to_markdown(), full.export_to_markdown());
}

#[test]
fn pdf_page_window_outside_document_is_an_error() {
    let err = DocumentConverter::new()
        .text_layer_only(true)
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

/// The layout + OCR models the force-OCR test needs. Model
/// resolution is CWD-relative (tests run from the crate dir) so, when the
/// repo-root copies exist, point the env overrides at them; skips cleanly on
/// a model-free checkout.
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

/// #429: the text detector reads what the layout model gives no region. On
/// `ModalNet-19.png` (a diagram from the Transformer paper) layout scores
/// only `SoftMax` above its 0.5 threshold, so recognition-only OCR emitted
/// that one word; docling reads every label through its OCR engine's
/// detector, and so does this pipeline with `.models/ocr_det.onnx` present
/// (skipped without it — recognition-only remains the documented fallback).
#[test]
fn text_detector_reads_labels_outside_layout_regions() {
    let det = repo_root().join(".models/ocr_det.onnx");
    if !ocr_models_ready() || !det.exists() {
        eprintln!("skipping: the OCR or text-detection models are not present");
        return;
    }
    std::env::set_var("DOCLING_OCR_DET_ONNX", &det);
    let src = SourceDocument::from_file(
        repo_root().join("tests/data/latex/sources/1706.03762/Figures/ModalNet-19.png"),
    )
    .expect("fixture");
    let md = DocumentConverter::new()
        .convert(src)
        .expect("convert")
        .document
        .export_to_markdown();
    for label in ["MatMul", "SoftMax", "Mask (opt.)", "Scale"] {
        assert!(md.contains(label), "{label:?} missing from {md:?}");
    }
    // Reading order: top to bottom, as RapidOCR sorts its boxes.
    let pos = |s: &str| md.find(s).unwrap();
    assert!(
        pos("SoftMax") < pos("Mask (opt.)") && pos("Mask (opt.)") < pos("Scale"),
        "{md:?}"
    );
}

/// `force_full_page_ocr` (docling's option of the same name) must actually
/// discard the text layer: converting a digital PDF page with it produces
/// OCR-recognized text, not the embedded cells. The fixture's text layer
/// spells "JavaScript Code Example" — OCR of the rendered page reads the same
/// heading, so the words prove the page converted *some* way; the
/// confidence report proves which way: an OCR score exists only when OCR
/// produced the page's text. (Comparing the two Markdown outputs proved
/// nothing: with the text detector — `DOCLING_OCR_DET_ONNX`, which another
/// test in this process sets — OCR reads the page exactly as the text layer
/// spells it, and the test failed whenever that test ran first.)
#[test]
fn force_full_page_ocr_discards_the_text_layer() {
    if !ocr_models_ready() {
        eprintln!("skipping: the OCR models are not present");
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
        .document;
    let forced = DocumentConverter::new()
        .page_range(1, 1)
        .force_full_page_ocr(true)
        .convert(src())
        .expect("forced convert")
        .document;
    let md = forced.export_to_markdown();
    assert!(
        md.contains("JavaScript"),
        "OCR should still read the page's heading: {md:?}"
    );
    let ocr_score = |doc: &docling_core::DoclingDocument| {
        doc.confidence
            .as_ref()
            .expect("pipeline sets confidence")
            .ocr_score()
    };
    assert_eq!(ocr_score(&normal), None, "digital page: nothing OCR'd");
    assert!(
        ocr_score(&forced).is_some(),
        "forced output must come from OCR, not the embedded text layer"
    );
}

/// `no_ocr` (#244 / #611, docling's `do_ocr=False`) never runs OCR — proven
/// with the OCR models *present*: a scanned page, whose only text is pixels,
/// converts to an empty document without an OCR score, while the same page
/// without the switch is read by OCR.
#[test]
fn no_ocr_never_runs_ocr_on_a_scan() {
    if !ocr_models_ready() {
        eprintln!("skipping: the OCR models are not present");
        return;
    }
    let src = || {
        SourceDocument::from_file(
            repo_root().join("tests/data/scanned/sources/ocr_test_raster.pdf"),
        )
        .expect("scanned fixture")
    };
    let ocr_score =
        |doc: &docling_core::DoclingDocument| doc.confidence.as_ref().and_then(|r| r.ocr_score());
    let read = DocumentConverter::new()
        .convert(src())
        .expect("default convert")
        .document;
    assert!(
        !read.export_to_markdown().trim().is_empty(),
        "OCR reads the scan"
    );
    assert!(ocr_score(&read).is_some());
    let skipped = DocumentConverter::new()
        .no_ocr(true)
        .convert(src())
        .expect("no_ocr on a scan must not error")
        .document;
    assert!(skipped.export_to_markdown().trim().is_empty());
    assert_eq!(ocr_score(&skipped), None, "no_ocr ran OCR");
}

/// #183: the PDF pipeline attaches a per-page confidence report to the
/// converted document. On a digital page the layout model scores real
/// clusters (well above the 0.3 label floor), the text layer parses cleanly
/// (parse_score ≈ 1.0), and nothing came from OCR — so ocr_score stays unset
/// and table_score is always unset (docling never assigns it either).
#[test]
fn pdf_pipeline_reports_confidence() {
    if !ocr_models_ready() {
        eprintln!("skipping: the OCR models are not present");
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
/// set (cell confidence 1.0), and OCR never ran. Needs no models.
#[test]
fn text_layer_only_conversion_reports_parse_confidence() {
    let doc = DocumentConverter::new()
        .text_layer_only(true)
        .page_range(1, 2)
        .convert(pdf_source())
        .expect("convert")
        .document;
    let report = doc.confidence.as_ref().expect("pipeline sets confidence");
    assert_eq!(report.pages.len(), 2, "one entry per converted page");
    assert!(report.pages.contains_key(&1) && report.pages.contains_key(&2));
    let parse = report.parse_score().expect("text layer parsed");
    assert!(parse > 0.5, "clean text layer, got {parse}");
    assert_eq!(report.ocr_score(), None, "no OCR on the text-layer path");
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

/// #238/#240: the ML table path emits first-class cells (text + page-point
/// bbox + span rectangle + OTSL header roles) on the public `Table`; the
/// JSON export serializes the real records (bbox + span offsets instead of
/// the synthesized 1×1 grid); and the bbox-driven repair API round-trips
/// into the re-export.
#[test]
fn table_cells_are_first_class_and_support_bbox_repair() {
    if !ocr_models_ready() || !tableformer_ready() {
        eprintln!("skipping: the ML models are not present");
        return;
    }
    let src =
        SourceDocument::from_file(repo_root().join("tests/data/pdf/sources/2305.03393v1-pg9.pdf"))
            .expect("table fixture");
    let mut document = DocumentConverter::new()
        .convert(src)
        .expect("convert")
        .document;

    // The JSON export carries the real cells: bboxes present.
    let json = document.export_to_json();
    assert!(
        json.contains("\"coord_origin\": \"TOPLEFT\""),
        "table cells serialize with bboxes"
    );

    let table = document
        .tables_mut()
        .find(|t| t.cells.is_some())
        .expect("the fixture's table goes through TableFormer and carries cells");
    let cells = table.cells.as_ref().unwrap();
    assert!(!cells.is_empty(), "first-class cells populated");
    assert!(
        cells.iter().all(|c| c.row_span >= 1 && c.col_span >= 1),
        "sane spans"
    );
    assert!(
        cells.iter().any(|c| c.bbox.is_some()),
        "cells carry geometry"
    );
    let (r, c, bbox) = cells
        .iter()
        .find(|c| !c.text.is_empty() && c.bbox.is_some())
        .map(|c| (c.start_row, c.start_col, c.bbox.unwrap()))
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

/// The heading-hierarchy stage (#302, docling's `HeadingHierarchyModel`) must
/// (a) leave output byte-identical when off — the default — and (b) actually
/// differentiate heading levels when on. The DocLayNet paper carries a real
/// PDF outline, so its major sections take bookmark level 1 (rendered `##`)
/// while unbookmarked front-matter headings fall to the style signal and land
/// deeper. The fixture pages are digital (no OCR runs), so the test needs
/// the layout model.
#[test]
fn heading_hierarchy_differentiates_levels_when_enabled() {
    if !ocr_models_ready() {
        eprintln!("skipping: the layout/OCR assets are not present");
        return;
    }
    let base = DocumentConverter::new()
        .page_range(1, 2)
        .convert(pdf_source())
        .expect("baseline convert")
        .document
        .export_to_markdown();
    let leveled = DocumentConverter::new()
        .page_range(1, 2)
        .heading_hierarchy(true)
        .convert(pdf_source())
        .expect("heading-hierarchy convert")
        .document
        .export_to_markdown();

    let levels = |md: &str| -> Vec<usize> {
        md.lines()
            .filter(|l| l.starts_with('#'))
            .map(|l| l.chars().take_while(|&c| c == '#').count())
            .collect()
    };
    // Off (the default): the flat legacy output — every heading at `##`.
    let base_levels = levels(&base);
    assert!(!base_levels.is_empty(), "fixture pages carry headings");
    assert!(
        base_levels.iter().all(|&d| d == 2),
        "default output keeps the flat ## levels: {base_levels:?}"
    );
    // On: bookmarks/style must produce at least two distinct depths, and the
    // heading *text* must be untouched (the stage only rewrites levels).
    let on_levels = levels(&leveled);
    assert_eq!(on_levels.len(), base_levels.len(), "no headings added/lost");
    assert!(
        on_levels
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            >= 2,
        "expected differentiated levels, got {on_levels:?}"
    );
    let strip = |md: &str| -> Vec<String> {
        md.lines()
            .filter(|l| l.starts_with('#'))
            .map(|l| l.trim_start_matches('#').trim().to_string())
            .collect()
    };
    assert_eq!(strip(&base), strip(&leveled), "heading text unchanged");
    // The bookmarked "1 INTRODUCTION" sits at the outline's top level (##).
    assert!(
        leveled.lines().any(|l| l == "## 1 INTRODUCTION"),
        "bookmark-matched heading takes outline level 1"
    );
}

/// `text_layer_only=auto` on a digital PDF: the text layer carries the
/// document, so the conversion is `text_layer_only=true`'s byte for byte —
/// and needs no model (the OCR model is not pointed at here, and the layout
/// model is never asked for).
#[test]
fn text_layer_auto_converts_a_digital_pdf_from_its_text_layer() {
    let on = DocumentConverter::new()
        .text_layer_only(true)
        .convert(pdf_source())
        .expect("text layer only")
        .document;
    let auto = DocumentConverter::new()
        .text_layer_only(TextLayerOnly::Auto)
        .convert(pdf_source())
        .expect("auto on a digital PDF")
        .document;
    assert!(!on.export_to_markdown().trim().is_empty());
    assert_eq!(on.export_to_markdown(), auto.export_to_markdown());
    assert_eq!(on.confidence, auto.confidence);
}

/// `text_layer_only=auto` on a scan: no usable text layer, so the full
/// pipeline runs and OCR reads the page — the default conversion's output,
/// where `text_layer_only=true` returns nothing. Needs the layout and OCR
/// models.
#[test]
fn text_layer_auto_runs_ocr_on_a_scan() {
    if !ocr_models_ready() {
        eprintln!("skipping: the OCR models are not present");
        return;
    }
    let src = || {
        SourceDocument::from_file(
            repo_root().join("tests/data/scanned/sources/ocr_test_raster.pdf"),
        )
        .expect("scanned fixture")
    };
    let md = |mode: TextLayerOnly| {
        DocumentConverter::new()
            .text_layer_only(mode)
            .convert(src())
            .expect("convert")
            .document
            .export_to_markdown()
    };
    let on = md(TextLayerOnly::On);
    assert!(
        on.trim().is_empty(),
        "the text path reads nothing off a scan"
    );
    let auto = md(TextLayerOnly::Auto);
    assert!(!auto.trim().is_empty(), "auto falls through to OCR");
    assert_eq!(
        auto,
        md(TextLayerOnly::Off),
        "auto on a scan is the default pipeline"
    );
}
