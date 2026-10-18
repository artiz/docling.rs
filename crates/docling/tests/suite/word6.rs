//! #640: a Word 6.0 / Word 95 `.doc` converts to the structure its `.docx`
//! twin has — tables, headings, bold runs, headers/footers — instead of a
//! flat run of body paragraphs. The fixtures are the issue's two files
//! (anonymised; Word 2016 can no longer write this format, so they cannot
//! be regenerated) with the `.docx` Word saved from each as the oracle.

use docling::{DoclingDocument, DocumentConverter, SourceDocument};
use docling_core::Node;
use std::path::PathBuf;

fn convert(format: &str, name: &str) -> DoclingDocument {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(format)
        .join("sources")
        .join(name);
    let source = SourceDocument::from_file(path).unwrap();
    DocumentConverter::new()
        .convert(source)
        .unwrap_or_else(|e| panic!("{name}: {e}"))
        .document
}

/// `(kind, text)` of the furniture-layer items: the paragraphs of the
/// `page header` / `page footer` sections (#697, the DOCX backend's shape)
/// and the labelled note texts.
fn furniture(doc: &DoclingDocument) -> Vec<(String, String)> {
    doc.nodes
        .iter()
        .flat_map(|n| match n {
            Node::Group {
                name: Some(name),
                layer: Some(docling_core::ContentLayer::Furniture),
                children,
                ..
            } => children
                .iter()
                .filter_map(|c| match c {
                    Node::Paragraph { text } => Some((name.clone(), text.clone())),
                    _ => None,
                })
                .collect(),
            Node::FurnitureText { label, text } => vec![(label.clone(), text.clone())],
            _ => Vec::new(),
        })
        .collect()
}

fn tables(doc: &DoclingDocument) -> Vec<(usize, usize)> {
    doc.nodes
        .iter()
        .filter_map(|n| match n {
            Node::Table(t) => Some((t.rows.len(), t.rows.first().map_or(0, |r| r.len()))),
            _ => None,
        })
        .collect()
}

/// Word 6.0 (`nFib` 101): two tables — one with a heading-styled cell and a
/// row merged across the columns, one whose cells hold several paragraphs
/// with blank lines between them — read as the `.docx` has them; the
/// Markdown is byte-identical.
#[test]
fn word6_tables_match_the_docx_twin() {
    let doc = convert("doc", "word6_tables.doc");
    let docx = convert("docx", "word6_tables.docx");
    assert_eq!(doc.export_to_markdown(), docx.export_to_markdown());
    assert_eq!(tables(&doc), vec![(3, 3), (2, 1)]);
    // The header's three cells and the footer: furniture, as Word 97's
    // are — the field results the file stores (Word re-evaluated them for
    // the `.docx`, so the texts differ). The first-page pair the file
    // keeps is not shown: the section has no title page (#697).
    let f = furniture(&doc);
    assert_eq!(
        f.iter().filter(|(l, _)| l == "page header").count(),
        3,
        "{f:?}"
    );
    assert_eq!(
        f.iter().filter(|(l, _)| l == "page footer").count(),
        1,
        "{f:?}"
    );
    assert!(f.iter().any(|(_, t)| t == "Alpha Br"), "{f:?}");
}

/// Word 95 (`nFib` 104): three tables, four headings (the built-in Heading
/// styles), bold runs — including a run whose `sprmCFBold` is "the
/// opposite of the style" in a bold heading-based style, which is *not*
/// bold — and one header / footer pair; the Markdown is byte-identical.
#[test]
fn word95_structures_match_the_docx_twin() {
    let doc = convert("doc", "word95_structures.doc");
    let docx = convert("docx", "word95_structures.docx");
    let md = doc.export_to_markdown();
    assert_eq!(md, docx.export_to_markdown());
    assert_eq!(tables(&doc), vec![(4, 2), (2, 1), (2, 1)]);
    let headings: Vec<(u8, &str)> = doc
        .nodes
        .iter()
        .filter_map(|n| match n {
            Node::Heading { level, text } => Some((*level, text.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        headings,
        vec![
            (2, "De\tEcho Fo"),
            (3, "Golf\tHotel Alpha Br"),
            (2, "Fo\tGolf Hotel"),
            (3, "Alph\tBravo Ch"),
        ]
    );
    assert!(md.contains("| Delta Echo Fox"), "{md}");
    assert!(!md.contains("**Gol**"), "{md}");
    let f = furniture(&doc);
    assert_eq!(
        f.iter().filter(|(l, _)| l == "page header").count(),
        3,
        "{f:?}"
    );
    assert_eq!(
        f.iter().filter(|(l, _)| l == "page footer").count(),
        1,
        "{f:?}"
    );
}

/// The pre-existing Word 6/95 fixtures still convert (no table pattern in
/// them, so their paragraphs stay paragraphs), and a truncated file keeps
/// giving text rather than an error.
#[test]
fn earlier_word6_fixtures_keep_converting() {
    for name in [
        "poi_word95.doc",
        "poi_word6_sections2.doc",
        "poi_word6_truncated.doc",
    ] {
        let doc = convert("doc", name);
        assert!(!doc.export_to_markdown().trim().is_empty(), "{name}");
        assert!(tables(&doc).is_empty(), "{name}: {:?}", tables(&doc));
    }
}
