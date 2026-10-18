//! #609: every text item of a layout-pipeline PDF carries its own `prov` in
//! the JSON, so a chunk made of it still knows its page and box. On the
//! reporter's two-page sample (`tests/fixtures/prov/`, a ReportLab document
//! with a bar chart, a table and a checkbox list) docling 2.135 gives every
//! text a `prov`; so must we. The chart's y-axis ticks `3`…`8`, each set by
//! its own `cm` under one shared `Tm`, are six picture children with their
//! own boxes there — the text layer used to glue them into one `345678`
//! cell carrying the first tick's box. And the checklist under "3. Options"
//! — one text block to the layout model — is four checkbox items, one per
//! drawn square.
//!
//! Needs the layout model only (`--no-ocr`), so it skips on a checkout
//! without `.models/`.

use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn layout_ready() -> bool {
    let m = repo_root().join(".models");
    let ready = ["layout_heron_int8.onnx", "layout_heron.onnx"]
        .iter()
        .any(|f| m.join(f).exists());
    ready && std::env::set_current_dir(repo_root()).is_ok()
}

#[test]
fn every_text_item_has_prov_and_axis_ticks_keep_their_boxes() {
    if !layout_ready() {
        eprintln!("skipping: the layout model is not present");
        return;
    }
    let path = repo_root().join("crates/docling/tests/fixtures/prov/issue609_sample.pdf");
    let doc = DocumentConverter::new()
        .no_ocr(true)
        .convert(SourceDocument::from_file(&path).expect("fixture"))
        .expect("conversion")
        .document;
    let json: serde_json::Value = serde_json::from_str(&doc.export_to_json()).unwrap();
    let texts = json["texts"].as_array().unwrap();

    let bare: Vec<&str> = texts
        .iter()
        .filter(|t| t["prov"].as_array().is_none_or(|p| p.is_empty()))
        .map(|t| t["text"].as_str().unwrap_or(""))
        .collect();
    assert!(bare.is_empty(), "text items without prov: {bare:?}");

    // The ticks hang under the chart picture, bottom (`3`) to top (`8`):
    // one item each, its box on its own baseline (y-up, so `t` rises).
    let ticks: Vec<(String, f64)> = json["pictures"][0]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let i: usize = c["$ref"]
                .as_str()
                .unwrap()
                .trim_start_matches("#/texts/")
                .parse()
                .unwrap();
            let t = &texts[i];
            let top = t["prov"][0]["bbox"]["t"].as_f64().unwrap();
            (t["text"].as_str().unwrap().to_string(), top)
        })
        .filter(|(s, _)| s.chars().all(|c| c.is_ascii_digit()))
        .collect();
    let labels: Vec<&str> = ticks.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(labels, ["3", "4", "5", "6", "7", "8"], "{ticks:?}");
    assert!(
        ticks.windows(2).all(|w| w[1].1 - w[0].1 > 20.0),
        "each tick sits ~26 pt above the last: {ticks:?}"
    );

    // The checklist: Heron reads the four options as one text block, but
    // each line has its own drawn square, so each is its own unchecked
    // checkbox item (docling's label, bare option text), top to bottom,
    // its box spanning square and label.
    let boxes: Vec<(&str, f64, f64)> = texts
        .iter()
        .filter(|t| t["label"] == "checkbox_unselected")
        .map(|t| {
            let b = &t["prov"][0]["bbox"];
            (
                t["text"].as_str().unwrap(),
                b["l"].as_f64().unwrap(),
                b["t"].as_f64().unwrap(),
            )
        })
        .collect();
    let labels: Vec<&str> = boxes.iter().map(|b| b.0).collect();
    assert_eq!(
        labels,
        [
            "First option",
            "Second option",
            "Third option",
            "Fourth option"
        ]
    );
    assert!(boxes.iter().all(|b| (b.1 - 119.5).abs() < 2.0), "{boxes:?}");
    assert!(boxes.windows(2).all(|w| w[0].2 > w[1].2), "{boxes:?}");
    let md = doc.export_to_markdown();
    assert!(
        md.contains("- [ ] First option\n\n- [ ] Second option"),
        "{md}"
    );
}

/// #609: checkboxes set as ballot-box glyphs (`tests/fixtures/prov/
/// checkbox_glyphs.pdf`, ReportLab with DejaVu Sans — regenerate with
/// `make_checkbox_glyphs.py` next to it). Heron labels the four lines
/// checkboxes itself but reads `☑ Bread` as unselected; docling 2.135 prints
/// `- [ ] ☑ Bread`. The glyph is the page's own record: each item's state
/// follows it and its label is the bare option text.
#[test]
fn ballot_box_glyphs_set_checkbox_state_and_leave_the_label() {
    if !layout_ready() {
        eprintln!("skipping: the layout model is not present");
        return;
    }
    let path = repo_root().join("crates/docling/tests/fixtures/prov/checkbox_glyphs.pdf");
    let doc = DocumentConverter::new()
        .no_ocr(true)
        .convert(SourceDocument::from_file(&path).expect("fixture"))
        .expect("conversion")
        .document;
    let json: serde_json::Value = serde_json::from_str(&doc.export_to_json()).unwrap();
    let items: Vec<(&str, &str)> = json["texts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["label"].as_str().unwrap().starts_with("checkbox_"))
        .map(|t| (t["label"].as_str().unwrap(), t["text"].as_str().unwrap()))
        .collect();
    assert_eq!(
        items,
        [
            ("checkbox_unselected", "Milk"),
            ("checkbox_selected", "Eggs"),
            ("checkbox_selected", "Bread"),
            ("checkbox_unselected", "Butter"),
        ]
    );
    let md = doc.export_to_markdown();
    assert!(
        md.contains("- [ ] Milk\n\n- [x] Eggs\n\n- [x] Bread\n\n- [ ] Butter"),
        "{md}"
    );
}
