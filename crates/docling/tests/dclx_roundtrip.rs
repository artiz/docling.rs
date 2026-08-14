//! #253 (docling-core 2.88/2.89 parity): DCLX round-trip robustness. The
//! writer-side fixes (XML-illegal markers, CDATA splitting, heading clamp)
//! are unit-tested in docling-core; these tests pin the full write → read
//! loop through the archive, including the reader behaviors docling fixed in
//! docling-core#689/#695 — our single-pass roxmltree reader never re-parses
//! text fragments as XML, so tag-shaped literals must survive as text.

use docling::{DocumentConverter, SourceDocument};
use docling_core::{DoclingDocument, Node, Table};

fn roundtrip(doc: &DoclingDocument) -> DoclingDocument {
    let bytes = docling::dclx::to_dclx_bytes(doc);
    let src = SourceDocument::from_bytes("t", docling::InputFormat::Dclx, bytes);
    DocumentConverter::new()
        .convert(src)
        .expect("dclx round-trip")
        .document
}

/// A literal `]]>` in body text survives the CDATA-section split
/// (docling-core#689's writer half) and reads back verbatim.
#[test]
fn cdata_delimiter_text_round_trips() {
    let mut doc = DoclingDocument::new("t");
    doc.push(Node::Paragraph {
        text: "a]]>b & c <t>".into(),
    });
    let back = roundtrip(&doc);
    assert_eq!(back.export_to_markdown(), doc.export_to_markdown());
}

/// Cell text that merely looks like DocLang/OTSL markup — docling-core#695's
/// literal `<fcel>`, plus a `<location .../>`-shaped string — stays text
/// through the archive instead of being interpreted as structure.
#[test]
fn tag_shaped_cell_text_round_trips() {
    let mut doc = DoclingDocument::new("t");
    doc.push(Node::Table(Table {
        rows: vec![
            vec!["h1".into(), "h2".into()],
            vec!["<fcel>".into(), "<location value=\"3\"/> x".into()],
        ],
        location: None,
        structure: None,
        cell_blocks: None,
        cells: None,
        caption: None,
    }));
    let back = roundtrip(&doc);
    let md = back.export_to_markdown();
    assert!(md.contains("&lt;fcel&gt;") || md.contains("<fcel>"), "{md}");
    assert_eq!(back.export_to_markdown(), doc.export_to_markdown());
}

/// XML-illegal control characters render as visible `[U+XXXX]` markers
/// (docling-core#687) — and, crucially, the archive stays parseable: the
/// round-trip must succeed rather than fail on invalid XML.
#[test]
fn xml_illegal_characters_round_trip_as_markers() {
    let mut doc = DoclingDocument::new("t");
    doc.push(Node::Paragraph {
        text: "break\u{0B}here".into(),
    });
    let back = roundtrip(&doc);
    assert!(
        back.export_to_markdown().contains("break[U+000B]here"),
        "{}",
        back.export_to_markdown()
    );
}

/// Deep headings clamp to level 6 (docling-core#688) and read back at 6.
#[test]
fn deep_headings_round_trip_clamped() {
    let mut doc = DoclingDocument::new("t");
    doc.push(Node::Heading {
        level: 42,
        text: "Deeper".into(),
    });
    let back = roundtrip(&doc);
    assert!(
        back.export_to_markdown().contains("###### Deeper"),
        "{}",
        back.export_to_markdown()
    );
}
