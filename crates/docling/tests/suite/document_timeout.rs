//! #497: `DocumentConverter::document_timeout` — docling's `document_timeout`.
//!
//! Runs on the `text_layer_only` path (text layer only, no models): the budget is
//! checked between pages regardless of what each page costs, so a budget of
//! one nanosecond is spent before the first page arrives and the conversion
//! returns an empty, partial document instead of failing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use docling::{ConversionError, ConversionStatus, DocumentConverter, SourceDocument};

fn pdf_source() -> SourceDocument {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/data/pdf/sources/2206.01062.pdf");
    SourceDocument::from_file(&path).expect("multi-page PDF fixture")
}

#[test]
fn a_spent_budget_is_a_partial_success_with_a_timeout_error() {
    let result = DocumentConverter::new()
        .text_layer_only(true)
        .document_timeout(Some(Duration::from_nanos(1)))
        .convert(pdf_source())
        .expect("a timeout is not a failure");
    assert_eq!(result.status, ConversionStatus::PartialSuccess);
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    let e = &result.errors[0];
    assert_eq!(e.component_type, "document_backend");
    assert_eq!(e.module_name, "pipeline");
    assert!(
        e.error_message.contains("document timeout") && e.error_message.contains("of 9 pages"),
        "{}",
        e.error_message
    );
    // Nothing fit the budget: the document is the pages processed — none.
    assert!(result.document.nodes.is_empty());
}

#[test]
fn an_ample_budget_converts_the_whole_document() {
    let whole = DocumentConverter::new()
        .text_layer_only(true)
        .convert(pdf_source())
        .expect("convert")
        .document;
    let result = DocumentConverter::new()
        .text_layer_only(true)
        .document_timeout(Some(Duration::from_secs(3600)))
        .convert(pdf_source())
        .expect("convert");
    assert_eq!(result.status, ConversionStatus::Success);
    assert!(result.errors.is_empty());
    assert_eq!(result.document.nodes.len(), whole.nodes.len());
}

/// The streaming conversion emits the pages that fit and ends with
/// `ConversionError::Timeout` as its last item — the chunk stream's spelling
/// of a partial success.
#[test]
fn a_streaming_conversion_ends_with_the_timeout_item() {
    let stream = DocumentConverter::new()
        .text_layer_only(true)
        .document_timeout(Some(Duration::from_nanos(1)))
        .convert_streaming(pdf_source())
        .expect("stream starts");
    let items: Vec<_> = stream.collect();
    let last = items.last().expect("at least the timeout item");
    assert!(
        matches!(last, Err(ConversionError::Timeout(_))),
        "last item: {last:?}"
    );
    assert!(
        items[..items.len() - 1].iter().all(|i| i.is_ok()),
        "every item before the timeout is a chunk"
    );
}

/// Declarative formats have no pages to stop between: the budget never
/// touches them (docling applies `document_timeout` to its paginated
/// pipeline only).
#[test]
fn declarative_formats_ignore_the_budget() {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/data/html/sources/hyperlink_03.html");
    let result = DocumentConverter::new()
        .document_timeout(Some(Duration::from_nanos(1)))
        .convert(SourceDocument::from_file(&path).expect("html fixture"))
        .expect("convert");
    assert_eq!(result.status, ConversionStatus::Success);
    assert!(result.errors.is_empty());
}
