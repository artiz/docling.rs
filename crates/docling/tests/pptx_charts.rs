//! PPTX charts against docling's own output: every plot of a combination
//! chart, ISO dates for date categories, the category axis title in the
//! table's corner (#756, docling#4773).
//!
//! `tests/data/pptx/charts/<name>.{md,json}` is **not** our output: it is
//! what docling 2.140.0's PowerPoint backend writes for `<name>.pptx`
//! (`MsPowerpointDocumentBackend(...).convert()`, then `export_to_markdown()`
//! and `export_to_dict()`). The decks are upstream's test decks, rebuilt by
//! `make_decks.py` next to them. Re-create a reference with upstream's
//! backend, never from our own output.
use std::fs;
use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};
use serde_json::Value;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/pptx/charts")
}

/// Drop what never depends on the conversion: the file's origin (hash, path)
/// and the schema version stamp.
fn normalize(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.remove("origin");
            m.remove("version");
            m.values_mut().for_each(normalize);
        }
        Value::Array(a) => a.iter_mut().for_each(normalize),
        _ => {}
    }
}

fn convert(name: &str) -> docling_core::DoclingDocument {
    let deck = dir().join(format!("{name}.pptx"));
    DocumentConverter::new()
        .convert(SourceDocument::from_file(&deck).expect("deck"))
        .unwrap_or_else(|e| panic!("{name}: {e}"))
        .document
}

/// Every deck's Markdown and JSON item tree equal docling's.
#[test]
fn charts_follow_docling() {
    let mut decks: Vec<String> = fs::read_dir(dir())
        .expect("charts fixtures")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "pptx"))
        .map(|p| p.file_stem().unwrap().to_str().unwrap().to_string())
        .collect();
    decks.sort();
    assert!(!decks.is_empty());
    let mut failures = Vec::new();
    for name in &decks {
        let document = convert(name);
        let base = dir().join(name);
        let want_md = fs::read_to_string(base.with_extension("md")).expect("reference .md");
        let got_md = document.export_to_markdown();
        if got_md.trim_end() != want_md.trim_end() {
            failures.push(format!(
                "{name}.md:\n--- docling\n{want_md}\n--- ours\n{got_md}"
            ));
        }
        let mut want: Value =
            serde_json::from_str(&fs::read_to_string(base.with_extension("json")).unwrap())
                .unwrap();
        let mut got: Value = serde_json::from_str(&document.export_to_json()).unwrap();
        normalize(&mut want);
        normalize(&mut got);
        if got != want {
            failures.push(format!("{name}.json: item tree differs from docling's"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// The combination chart's two plot types, its six series and its ISO
/// dates, read through the public API.
#[test]
fn a_combination_chart_keeps_every_plot() {
    let doc = convert("combo_line");
    let json: Value = serde_json::from_str(&doc.export_to_json()).unwrap();
    let pic = &json["pictures"][0];
    let classes: Vec<&str> = pic["meta"]["classification"]["predictions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["class_name"].as_str().unwrap())
        .collect();
    assert_eq!(classes, ["bar_chart", "line_chart"]);
    let grid = &pic["meta"]["tabular_chart"]["chart_data"]["grid"];
    let header: Vec<&str> = grid[0]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        header,
        [
            "Region Position",
            "East",
            "West",
            "North",
            "South",
            "Margin",
            "Target"
        ]
    );
    assert_eq!(grid[1][0]["text"], "2012-01-01");
    assert_eq!(grid[1][5]["text"], "0.126");
}
