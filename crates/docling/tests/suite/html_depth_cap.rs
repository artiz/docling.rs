//! #912: an HTML page nested past `DOCLING_RS_MAX_HTML_DEPTH` is still
//! converted — as flat text, without the item tree and the source blocks —
//! but the conversion says so: a `Notice` on the document naming the
//! variable and the effective cap, and a `partial_success` with the same
//! message in the result's errors, so no caller mistakes the flattened
//! text for a document with source identity. At the default cap (2000), so
//! no environment variable is touched.

use docling::{ConversionStatus, DocumentConverter, InputFormat, SourceDocument};
use docling_core::NoticeKind;

#[test]
fn a_page_past_the_depth_cap_names_the_cap() {
    let depth = 2100;
    let html = format!(
        "<html><body>{}<p>deep text</p>{}</body></html>",
        "<div>".repeat(depth),
        "</div>".repeat(depth),
    );
    let result = DocumentConverter::new()
        .convert(SourceDocument::from_bytes(
            "deep.html",
            InputFormat::Html,
            html.into_bytes(),
        ))
        .expect("a deep page still converts");
    let doc = &result.document;
    assert!(doc.export_to_markdown().contains("deep text"));
    assert!(doc.tree.is_none(), "no DOM walk, no item tree");
    assert_eq!(
        doc.notices.iter().map(|n| &n.kind).collect::<Vec<_>>(),
        vec![&NoticeKind::DepthLimit {
            variable: "DOCLING_RS_MAX_HTML_DEPTH",
            limit: 2000
        }]
    );
    assert_eq!(result.status, ConversionStatus::PartialSuccess);
    let [error] = result.errors.as_slice() else {
        panic!("{:?}", result.errors);
    };
    assert_eq!(error.module_name, "DOCLING_RS_MAX_HTML_DEPTH");
    assert!(
        error.error_message.contains("2000") && error.error_message.contains("source blocks"),
        "{}",
        error.error_message
    );

    // A page within the cap: a tree, source blocks, a plain success.
    let result = DocumentConverter::new()
        .convert(SourceDocument::from_bytes(
            "shallow.html",
            InputFormat::Html,
            b"<html><body><div><p>shallow</p></div></body></html>".to_vec(),
        ))
        .unwrap();
    assert_eq!(result.status, ConversionStatus::Success);
    assert!(result.document.notices.is_empty());
    assert!(!result.document.tree.as_ref().unwrap().blocks.is_empty());
}
