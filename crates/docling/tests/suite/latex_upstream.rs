//! DocLang of the LaTeX corpus against docling's own (#884): the backend's
//! item tree serialized as docling-core's `DocLangDocSerializer` does it.
//!
//! `tests/data/latex/groundtruth/<name>.dclg` is docling 2.142.1's
//! `export_to_doclang()` of the source (docling-core's default placeholder
//! image mode, hence `export_to_doclang_with_images(Placeholder)` here) —
//! upstream's fixture, mirrored, never our output.
use std::fs;
use std::path::Path;

use docling::{DocumentConverter, ImageMode, SourceDocument};

#[test]
fn doclang_follows_docling() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = crate_dir.join("../../tests/data/latex");
    let mirror = fs::read_to_string(crate_dir.join("tests/data/latex/mirror.txt"))
        .expect("latex mirror list");
    let mut failures = Vec::new();
    for rel in mirror.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let name = rel.replace('/', "_");
        let want = fs::read_to_string(root.join("groundtruth").join(format!("{name}.dclg")))
            .unwrap_or_else(|e| panic!("{name}.dclg: {e}"));
        let got = DocumentConverter::new()
            .convert(SourceDocument::from_file(root.join("sources").join(rel)).expect("source"))
            .unwrap_or_else(|e| panic!("{rel}: {e}"))
            .document
            .export_to_doclang_with_images(ImageMode::Placeholder)
            .0;
        if got != want {
            let line = got
                .lines()
                .zip(want.lines())
                .position(|(g, w)| g != w)
                .map_or(got.lines().count().min(want.lines().count()), |i| i)
                + 1;
            failures.push(format!("{name} (first difference on line {line})"));
        }
    }
    assert!(
        failures.is_empty(),
        "DocLang differs from docling: {failures:?}"
    );
}
