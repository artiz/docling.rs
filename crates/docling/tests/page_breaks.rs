//! `DocumentConverter::page_break_placeholder` — docling's
//! `export_to_markdown(page_break_placeholder=…)` — end to end over formats
//! that carry page boundaries: DjVu pages (`PageBreak`), slides (`PageInfo`
//! + `PageBreak`), sheets. Declarative only, so it runs on every build.

use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};

const PB: &str = "<!-- page break -->";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn markdown(src: &Path, placeholder: Option<&str>) -> String {
    let source = SourceDocument::from_file(src).expect("fixture");
    DocumentConverter::new()
        .page_break_placeholder(placeholder.map(str::to_owned))
        .convert(source)
        .expect("convert")
        .document
        .export_to_markdown()
}

/// The placeholder is additive: dropping the inserted parts gives back the
/// default output byte for byte, and no part leads or trails the document.
fn assert_additive(with: &str, without: &str, expected_breaks: usize) {
    assert_eq!(
        with.matches(PB).count(),
        expected_breaks,
        "page breaks in:\n{with}"
    );
    assert_eq!(with.replace(&format!("{PB}\n\n"), ""), without);
    assert!(!with.starts_with(PB), "leading break:\n{with}");
    assert!(!with.trim_end().ends_with(PB), "trailing break:\n{with}");
}

#[test]
fn djvu_pages_are_separated_by_the_placeholder() {
    // Three pages, each with a text layer — the DjVu backend emits a
    // `PageBreak` between consecutive pages.
    let src = root().join("crates/docling/tests/data/djvu/sources/example.djvu");
    let plain = markdown(&src, None);
    assert!(!plain.contains(PB));
    let with = markdown(&src, Some(PB));
    assert_additive(&with, &plain, 2);
}

#[test]
fn a_page_window_still_breaks_only_inside_the_window() {
    let src = root().join("crates/docling/tests/data/djvu/sources/sample1.djvu");
    let source = SourceDocument::from_file(&src).expect("fixture");
    let doc = DocumentConverter::new()
        .page_range(2, 3)
        .page_break_placeholder(Some(PB.into()))
        .convert(source)
        .expect("convert")
        .document;
    let md = doc.export_to_markdown();
    assert_eq!(md.matches(PB).count(), 1, "{md}");
}

#[test]
fn slides_are_pages_too() {
    // PPTX emits a `PageInfo` marker per slide *and* a `PageBreak` between
    // slides; the pair must collapse into one break.
    let src = root().join("tests/data/pptx/sources/powerpoint_sample.pptx");
    let plain = markdown(&src, None);
    let with = markdown(&src, Some(PB));
    let breaks = with.matches(PB).count();
    assert!(breaks >= 1, "expected at least one slide boundary:\n{with}");
    assert_additive(&with, &plain, breaks);
    assert!(
        !with.contains(&format!("{PB}\n\n{PB}")),
        "doubled break at a slide boundary:\n{with}"
    );
}

#[test]
fn streamed_markdown_matches_the_buffered_export() {
    let src = root().join("crates/docling/tests/data/djvu/sources/example.djvu");
    let buffered = markdown(&src, Some(PB));
    let source = SourceDocument::from_file(&src).expect("fixture");
    let streamed: String = DocumentConverter::new()
        .page_break_placeholder(Some(PB.into()))
        .convert_streaming(source)
        .expect("stream")
        .map(|chunk| chunk.expect("chunk"))
        .collect();
    assert_eq!(streamed, buffered);
}

#[test]
fn an_unpaged_document_is_untouched() {
    let src = root().join("tests/data/md/sources/inline_and_formatting.md");
    let plain = markdown(&src, None);
    let with = markdown(&src, Some(PB));
    assert_eq!(with, plain);
}
