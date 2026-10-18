//! ODF against docling's own output, for the documents of upstream tests
//! that docling.rs ports: hidden sections and slides (#753, docling#4767 and
//! docling#4860), pictures anchored to a sheet (#758, docling#4678), list
//! items holding only an image, a chart or a note (#759, docling#4693).
//!
//! `tests/data/odf/upstream/<name>.{md,json}` is **not** our output: it is
//! what docling 2.140.0's OpenDocument backends write for the document
//! (`Od{t,s,p}DocumentBackend(...).convert()`, then `export_to_markdown()`
//! and `export_to_dict()`). The documents are upstream's test documents,
//! rebuilt by `make_docs.py` next to them. Re-create a reference with
//! upstream's backend, never from our own output.
use std::fs;
use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};
use serde_json::Value;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/odf/upstream")
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

fn convert(name: &str) -> docling_core::DoclingDocument {
    let path = dir().join(name);
    DocumentConverter::new()
        .convert(SourceDocument::from_file(&path).expect("document"))
        .unwrap_or_else(|e| panic!("{name}: {e}"))
        .document
}

/// Every document's Markdown and JSON item tree equal docling's.
#[test]
fn documents_follow_docling() {
    let mut docs: Vec<String> = fs::read_dir(dir())
        .expect("odf/upstream fixtures")
        .map(|e| e.expect("entry").path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e == "odt" || e == "ods" || e == "odp")
        })
        .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
        .collect();
    docs.sort();
    assert!(!docs.is_empty());
    let mut failures = Vec::new();
    for name in &docs {
        let document = convert(name);
        let base = dir().join(name);
        let want_md = fs::read_to_string(base.with_extension("md")).expect("reference .md");
        let got_md = document.export_to_markdown();
        if got_md.trim_end() != want_md.trim_end() {
            failures.push(format!(
                "{name} md:\n--- docling\n{want_md}\n--- ours\n{got_md}"
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
            failures.push(format!("{name} json: item tree differs from docling's"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
