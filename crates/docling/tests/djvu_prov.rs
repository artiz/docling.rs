//! DjVu page provenance in the JSON export: each page's `PageInfo` marker
//! becomes docling's `pages` entry and each paragraph's text-layer zone box
//! its `prov` — like a PDF, so docling-core's `pages` filter and
//! `export_to_markdown(page_break_placeholder=…)` work on a DjVu's JSON.
//! Pure-Rust decode, so it runs on every build.

use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};
use serde_json::Value;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/djvu/sources/example.djvu")
}

fn json(converter: DocumentConverter) -> Value {
    let source = SourceDocument::from_file(fixture()).expect("fixture");
    let doc = converter.convert(source).expect("convert").document;
    serde_json::from_str(&doc.export_to_json()).expect("valid JSON")
}

/// `pages` lists every page with its size in points (5100×6600 px at 600 dpi
/// is US Letter), and every text item carries one prov entry on its page
/// with a BOTTOMLEFT bbox inside the page box and a charspan over its text.
#[test]
fn djvu_json_carries_pages_and_prov() {
    let v = json(DocumentConverter::new());
    let pages = v["pages"].as_object().expect("pages map");
    assert_eq!(pages.len(), 3, "{pages:?}");
    for (no, page) in pages {
        assert_eq!(page["page_no"].as_u64().unwrap().to_string(), *no);
        assert_eq!(page["size"]["width"], 612.0);
        assert_eq!(page["size"]["height"], 792.0);
    }
    let texts = v["texts"].as_array().expect("texts");
    assert_eq!(texts.len(), 3, "one reflowed paragraph per page");
    for (i, t) in texts.iter().enumerate() {
        let prov = &t["prov"];
        assert_eq!(prov.as_array().map(Vec::len), Some(1), "{t}");
        let p = &prov[0];
        assert_eq!(p["page_no"], (i + 1) as u64);
        let b = &p["bbox"];
        assert_eq!(b["coord_origin"], "BOTTOMLEFT");
        let (l, t_, r, bt) = (
            b["l"].as_f64().unwrap(),
            b["t"].as_f64().unwrap(),
            b["r"].as_f64().unwrap(),
            b["b"].as_f64().unwrap(),
        );
        assert!(0.0 <= l && l < r && r <= 612.0, "{b}");
        assert!(0.0 <= bt && bt < t_ && t_ <= 792.0, "{b}");
        let len = t["text"].as_str().unwrap().chars().count() as u64;
        assert_eq!(p["charspan"], serde_json::json!([0, len]));
    }
}

/// A `--pages` window keeps the real page numbers, as the PDF path does.
#[test]
fn djvu_page_window_keeps_real_page_numbers() {
    let v = json(DocumentConverter::new().page_range(2, 3));
    let pages = v["pages"].as_object().expect("pages map");
    assert_eq!(pages.keys().collect::<Vec<_>>(), ["2", "3"]);
    let nos: Vec<u64> = v["texts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["prov"][0]["page_no"].as_u64().unwrap())
        .collect();
    assert_eq!(nos, [2, 3]);
}
