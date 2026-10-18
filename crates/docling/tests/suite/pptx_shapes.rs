//! PPTX shape geometry and links against docling's own output: shapes
//! inside a moved, resized, nested or flipped group (#766, docling#4859).
//!
//! `tests/data/pptx/shapes/<name>.{md,json}` is **not** our output: it is
//! what docling 2.140.0's PowerPoint backend writes for `<name>.pptx`
//! (`MsPowerpointDocumentBackend(...).convert()`, then `export_to_markdown()`
//! and `export_to_dict()`). The decks are upstream's test decks, rebuilt by
//! `make_decks.py` next to them. Re-create a reference with upstream's
//! backend, never from our own output.
use std::fs;
use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};
use serde_json::Value;

const DECKS: &[&str] = &["group_bbox"];

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/pptx/shapes")
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

/// Every deck's Markdown and JSON item tree (provenance boxes and links
/// included) equal docling's.
#[test]
fn shapes_follow_docling() {
    let mut failures = Vec::new();
    for name in DECKS {
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
            failures.push(format!(
                "{name}.json: item tree differs from docling's\n--- docling\n{}\n--- ours\n{}",
                serde_json::to_string_pretty(&want["texts"]).unwrap(),
                serde_json::to_string_pretty(&got["texts"]).unwrap()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// The issue's case in slide inches: a text box in a group moved to
/// (4in, 3in) is drawn there, not at its child-space `(1in, 2in)`.
#[test]
fn a_moved_group_moves_its_shapes() {
    let document = convert("group_bbox");
    let json: Value = serde_json::from_str(&document.export_to_json()).unwrap();
    let moved = json["texts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["text"] == "moved")
        .expect("moved text");
    let bbox = &moved["prov"][0]["bbox"];
    let inch = |v: &Value| v.as_f64().unwrap() / 914_400.0;
    assert_eq!(
        [
            inch(&bbox["l"]),
            inch(&bbox["t"]),
            inch(&bbox["r"]),
            inch(&bbox["b"])
        ],
        [4.0, 3.0, 5.0, 4.0]
    );
}
