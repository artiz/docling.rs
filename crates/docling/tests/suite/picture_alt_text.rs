//! Picture alt text (#676): the `descr` / `title` of a DOCX, PPTX or XLSX
//! picture's non-visual properties, kept as the picture's description with
//! `created_by = "alt_text"` — always in the JSON, in Markdown and HTML only
//! under strict Markdown or `picture_alt_text`.

use std::path::{Path, PathBuf};

use docling::{DocumentConverter, ImageMode, SourceDocument};
use docling_core::{Node, PictureDescription};

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// MarkItDown's `test.pptx` (MIT), whose slide 6 picture carries an
/// author-written alt text.
const PPTX: &str = "crates/docling-py/tests/data/markitdown/test.pptx";
const ALT: &str = "This phrase of the caption is Human-written.";

fn convert(rel: &str, converter: DocumentConverter) -> docling_core::DoclingDocument {
    let source = SourceDocument::from_file(repo(rel)).expect("fixture");
    converter.convert(source).expect("converts").document
}

fn alt_texts(doc: &docling_core::DoclingDocument) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(nodes: &[Node], out: &mut Vec<String>) {
        for n in nodes {
            match n {
                Node::Picture {
                    description: Some(d),
                    ..
                } if d.is_alt_text() => out.push(d.text.clone()),
                Node::Group { children, .. } => walk(children, out),
                Node::Located { inner, .. } | Node::Prov { inner, .. } => {
                    walk(std::slice::from_ref(inner), out)
                }
                _ => {}
            }
        }
    }
    walk(&doc.nodes, &mut out);
    out
}

/// The issue's acceptance on `test.pptx`: the alt string is in the JSON,
/// in Markdown under the flag (and under strict), not in the default.
#[test]
fn pptx_alt_text_is_in_json_and_on_request_in_markdown() {
    let doc = convert(PPTX, DocumentConverter::new());
    assert!(
        alt_texts(&doc).iter().any(|t| t == ALT),
        "{:?}",
        alt_texts(&doc)
    );

    let json = doc.export_to_json();
    assert!(json.contains(ALT), "{json}");
    assert!(json.contains("\"created_by\": \"alt_text\""), "{json}");

    let md = doc.export_to_markdown();
    assert!(!md.contains(ALT), "default Markdown must not change:\n{md}");
    assert!(doc.export_to_markdown_with(true).contains(ALT));

    let flagged = convert(PPTX, DocumentConverter::new().picture_alt_text(true));
    let md = flagged.export_to_markdown();
    assert!(md.contains(ALT), "{md}");
    // With the placeholder the alt text is the line above the marker; with
    // the image data it is the image line's alt.
    assert!(md.contains(&format!("{ALT}\n\n<!-- image -->")), "{md}");
    let embedded = flagged
        .export_to_markdown_with_images(ImageMode::Embedded, "artifacts")
        .0;
    assert!(
        embedded.contains(&format!("![{ALT}](data:image/")),
        "{embedded}"
    );
    assert!(!embedded.contains(&format!("{ALT}\n\n![")), "{embedded}");

    // HTML: nothing by default (docling's HTML byte for byte); under the
    // flag the `<img alt>` and the description text.
    let (html, _) = doc.export_to_html_with_images(ImageMode::Embedded, "artifacts");
    assert!(!html.contains(ALT), "{html}");
    let (html, _) = flagged.export_to_html_with_images(ImageMode::Embedded, "artifacts");
    assert!(html.contains(&format!("alt=\"{ALT}\"")), "{html}");
    assert!(html.matches(ALT).count() >= 2, "{html}");
}

/// DOCX: Word's "Alt Text" lives on the drawing's `wp:docPr`; both chains
/// (the flat nodes and the JSON tree) carry it. XLSX: the drawing's
/// `xdr:cNvPr`.
#[test]
fn docx_and_xlsx_alt_text_reach_the_json() {
    let docx = convert(
        "crates/docling/tests/data/docx/sources/pandoc_images.docx",
        DocumentConverter::new(),
    );
    let alts = alt_texts(&docx);
    for want in ["a red square", "a green square", "a metafile"] {
        assert!(alts.iter().any(|t| t == want), "{alts:?}");
    }
    let json = docx.export_to_json();
    assert!(json.contains("\"text\": \"a red square\""), "{json}");
    assert!(!docx.export_to_markdown().contains("a red square"));
    assert!(docx.export_to_markdown_with(true).contains("a red square"));

    let xlsx = convert(
        "tests/data/xlsx/sources/xlsx_emf.xlsx",
        DocumentConverter::new().picture_alt_text(true),
    );
    let alts = alt_texts(&xlsx);
    assert!(
        alts.iter().any(|t| t.starts_with("A cartoon duck")),
        "{alts:?}"
    );
    assert!(xlsx.export_to_markdown().contains("A cartoon duck"));
}

/// A picture without alt text, or with a blank one, has no description;
/// the alt text's `]` and line breaks cannot break the image line.
#[test]
fn image_line_alt_is_one_safe_line() {
    let mut doc = docling_core::DoclingDocument::new("t");
    doc.picture_alt_text = true;
    doc.push(Node::Picture {
        caption: None,
        caption_href: None,
        image: Some(docling_core::PictureImage {
            mimetype: "image/png".into(),
            width: 1,
            height: 1,
            dpi: 72,
            data: vec![
                0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44,
                0x52, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D,
                0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xFF, 0xFF, 0x3F, 0, 5, 0xFE, 2,
                0xFE, 0xA3, 0x4A, 0x4E, 0x1C, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60,
                0x82,
            ],
            display_size: None,
        }),
        classification: None,
        description: Some(PictureDescription::alt_text("Owner [avatar]\n\nline two")),
        caption_parent: Default::default(),
        caption_location: None,
    });
    let md = doc
        .export_to_markdown_with_images(ImageMode::Embedded, "artifacts")
        .0;
    assert!(md.starts_with("![Owner [avatar\\] line two](data:"), "{md}");
    assert!(PictureDescription::alt_text("x").is_alt_text());
}
