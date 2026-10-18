//! PPTX list-marker cascade (#627) against docling's own output.
//!
//! `tests/data/pptx/list_cascade/<name>.{md,json}` is **not** our output: it
//! is what Python docling writes for `<name>.pptx` with the PowerPoint backend
//! of upstream `main` at b2a59b9 (2.135.0 + docling#4581, "respect layout
//! bullet styles", which no release carries yet) —
//!
//! ```python
//! doc = DocumentConverter().convert("<name>.pptx").document
//! doc.export_to_markdown(); doc.export_to_dict()
//! ```
//!
//! A–H are the reporter's decks (`make_repro.py` in #627: the python-pptx
//! default template with one layout or master list style edited); their
//! Python 2.135.0-14 Markdown is byte-identical to these files. I–K come from
//! `make_decks.py` next to them: a slide placeholder whose `idx` has no layout
//! counterpart (docling then never reaches the master), levels resolved at
//! different steps of the chain, and a numbered list's `startAt` with a bullet
//! joining the numbered group. Re-create a reference with upstream's backend,
//! never from our own output.
//!
//! `tests/data/ppt/list_cascade/` holds the reporter's A–G decks as saved by
//! PowerPoint (for Mac) in the 97–2003 format. docling reads a `.ppt` by
//! letting LibreOffice save it as `.pptx` and running the same backend, so
//! each `.md` is that chain's output (LibreOffice 24.2; byte-identical to
//! the reporter's docling 2.134 run): the bullets PowerPoint leaves to the
//! master's `TextMasterStyleAtom`, and the slide's shapes in reading order.
//!
//! `tests/data/ppt/list_cascade_lo/` holds all eleven decks as LibreOffice
//! 24.2 saves them (`soffice --headless --convert-to ppt`, the thumbnail
//! property streams dropped to keep each under 60 KB — the `PowerPoint
//! Document` stream is untouched). LibreOffice writes every paragraph's
//! *resolved* bullet (its own `StyleTextPropAtom` run, PP9 autonumbers for
//! the numbered ones) and a master per layout, so these exercise the
//! per-paragraph path and docling's list stack (J: a level-2 numbered run
//! after plain text is a flat list; K: a bullet joining a numbered group
//! takes its numbering) rather than the master inheritance. Each `.md` is
//! docling 2.137.0's output for the file (LibreOffice 24.2 underneath).
use std::fs;
use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};
use serde_json::Value;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/pptx/list_cascade")
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

#[test]
fn list_markers_follow_docling_through_layout_and_master() {
    let converter = DocumentConverter::new();
    let mut decks: Vec<PathBuf> = fs::read_dir(dir())
        .expect("list_cascade fixtures")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "pptx"))
        .collect();
    decks.sort();
    assert_eq!(decks.len(), 11, "the A–K decks");
    let mut failures = Vec::new();
    for deck in &decks {
        let name = deck.file_stem().unwrap().to_str().unwrap();
        let document = converter
            .convert(SourceDocument::from_file(deck).expect("deck"))
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .document;

        let want_md = fs::read_to_string(deck.with_extension("md")).expect("reference .md");
        let got_md = document.export_to_markdown();
        if got_md.trim_end() != want_md.trim_end() {
            failures.push(format!(
                "{name}.md:\n--- docling\n{want_md}\n--- ours\n{got_md}"
            ));
        }

        let mut want: Value =
            serde_json::from_str(&fs::read_to_string(deck.with_extension("json")).unwrap())
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

#[test]
fn ppt_list_markers_follow_docling_through_the_master() {
    let converter = DocumentConverter::new();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ppt/list_cascade");
    let mut decks: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("ppt list_cascade fixtures")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "ppt"))
        .collect();
    decks.sort();
    assert_eq!(decks.len(), 7, "the A–G decks");
    let mut failures = Vec::new();
    for deck in &decks {
        let name = deck.file_stem().unwrap().to_str().unwrap();
        let got = converter
            .convert(SourceDocument::from_file(deck).expect("deck"))
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .document
            .export_to_markdown();
        let want = fs::read_to_string(deck.with_extension("md")).expect("reference .md");
        if got.trim_end() != want.trim_end() {
            failures.push(format!("{name}.md:\n--- docling\n{want}\n--- ours\n{got}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn libreoffice_saved_ppt_list_markers_follow_docling() {
    let converter = DocumentConverter::new();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ppt/list_cascade_lo");
    let mut decks: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("ppt list_cascade_lo fixtures")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "ppt"))
        .collect();
    decks.sort();
    assert_eq!(decks.len(), 11, "the A–K decks");
    let mut failures = Vec::new();
    for deck in &decks {
        let name = deck.file_stem().unwrap().to_str().unwrap();
        let got = converter
            .convert(SourceDocument::from_file(deck).expect("deck"))
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .document
            .export_to_markdown();
        let want = fs::read_to_string(deck.with_extension("md")).expect("reference .md");
        if got.trim_end() != want.trim_end() {
            failures.push(format!("{name}.md:\n--- docling\n{want}\n--- ours\n{got}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
