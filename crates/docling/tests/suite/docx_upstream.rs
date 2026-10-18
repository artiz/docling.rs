//! DOCX against docling's own output, for the documents of upstream tests
//! that docling.rs ports: text boxes in table cells (#757, docling#4757),
//! list items resumed in a table cell (#764, docling#4674), lists numbered
//! through a linked numbering style (#765, docling#4727).
//!
//! `tests/data/docx/upstream/<name>.{md,json}` is **not** our output: it is
//! what docling 2.140.0's Word backend writes for `<name>.docx`
//! (`MsWordDocumentBackend(...).convert()`, then `export_to_markdown()` and
//! `export_to_dict()`). The documents are upstream's test documents, rebuilt
//! by `make_docs.py` next to them. Re-create a reference with upstream's
//! backend, never from our own output.
use std::fs;
use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};
use serde_json::Value;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/docx/upstream")
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

/// docling.rs keeps a picture's alt text as its description (#676,
/// `created_by: "alt_text"`); docling does not. a picture's alt text is the one
/// deliberate difference these documents can show.
fn drop_alt_text(doc: &mut Value) {
    for pic in doc["pictures"].as_array_mut().into_iter().flatten() {
        let alt = pic["meta"]["description"]["created_by"] == "alt_text";
        if alt {
            pic.as_object_mut().unwrap().remove("meta");
            pic["annotations"]
                .as_array_mut()
                .unwrap()
                .retain(|a| a["provenance"] != "alt_text");
        }
    }
}

/// Documents compared through upstream's own assertions
/// ([`a_textbox_in_a_cell_is_emitted_once_inside_it`]) rather than its full
/// output. `textbox_cell_word_mc` anchors its textbox in a DrawingML
/// drawing: docling renders such a drawing through LibreOffice (a blank one
/// lands on the invisible layer) or drops it when LibreOffice is missing;
/// docling.rs keeps it as a picture without image data — a standing
/// difference of the DrawingML path, not of the textbox handling.
const OWN_ASSERTIONS: &[&str] = &["textbox_cell_word_mc"];

fn convert(name: &str) -> docling_core::DoclingDocument {
    let path = dir().join(format!("{name}.docx"));
    DocumentConverter::new()
        .convert(SourceDocument::from_file(&path).expect("document"))
        .unwrap_or_else(|e| panic!("{name}: {e}"))
        .document
}

/// Every document's Markdown and JSON item tree equal docling's.
#[test]
fn documents_follow_docling() {
    let mut docs: Vec<String> = fs::read_dir(dir())
        .expect("docx/upstream fixtures")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "docx"))
        .map(|p| p.file_stem().unwrap().to_str().unwrap().to_string())
        .collect();
    docs.sort();
    assert!(!docs.is_empty());
    let mut failures = Vec::new();
    for name in docs
        .iter()
        .filter(|n| !OWN_ASSERTIONS.contains(&n.as_str()))
    {
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
        drop_alt_text(&mut got);
        if got != want {
            failures.push(format!("{name}.json: item tree differs from docling's"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// docling's `test_textbox_in_table_cell_is_emitted_once_inside_cell`
/// (#757, docling#4757): the textbox text appears once, inside its cell.
#[test]
fn a_textbox_in_a_cell_is_emitted_once_inside_it() {
    for (name, marker) in [
        ("textbox_cell_vml_single_para", "MARKA"),
        ("textbox_cell_vml_multi_para", "MARKA"),
        ("textbox_cell_word_mc", "MARKW"),
    ] {
        let doc = convert(name);
        let md = doc.export_to_markdown();
        assert_eq!(md.matches(marker).count(), 1, "{name}: {md}");
        assert!(
            md.lines()
                .any(|l| l.starts_with('|') && l.contains(marker) && l.contains('b')),
            "{name}: {md}"
        );
        let json: Value = serde_json::from_str(&doc.export_to_json()).unwrap();
        let texts: Vec<&str> = json["texts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["text"].as_str())
            .filter(|t| !t.trim().is_empty())
            .collect();
        assert_eq!(texts, ["debut", marker, "fin"], "{name}");
    }
}
