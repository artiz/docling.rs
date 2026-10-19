//! #783: `DocumentConverter::progress_callback` — docling's conversion
//! progress events (docling#4560).
//!
//! The event flow runs on the `text_layer_only` path (no models): the page
//! walks report every page through the same hook the ML pipeline uses, and a
//! one-nanosecond budget is spent before the first page, so the cut-off
//! reporting is deterministic. The last test runs the real pipeline and
//! skips cleanly without the models, like `scanned.rs`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use docling::{
    ConversionStatus, DocumentConverter, ProgressCallback, ProgressEvent, ProgressKind,
    SourceDocument,
};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A 3-page PDF with a text layer.
fn three_pages() -> SourceDocument {
    SourceDocument::from_file(repo_root().join("tests/data/pdf/sources/skipped_1page.pdf"))
        .expect("fixture")
}

fn ml_stack_ready() -> bool {
    let root = repo_root();
    let models_ready = ["layout_heron.onnx", "ocr_rec.onnx", "ppocr_keys_v1.txt"]
        .iter()
        .all(|m| root.join(".models").join(m).exists());
    models_ready && std::env::set_current_dir(&root).is_ok()
}

type Seen = Arc<Mutex<Vec<ProgressEvent>>>;

fn recorder() -> (ProgressCallback, Seen) {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let callback: ProgressCallback = Arc::new(move |e| sink.lock().unwrap().push(e.clone()));
    (callback, seen)
}

/// `(page_no, success, completed_pages, total_pages)` of the page events.
fn pages(events: &[ProgressEvent]) -> Vec<(usize, bool, usize, usize)> {
    events
        .iter()
        .filter_map(|e| match e.kind {
            ProgressKind::PageCompleted {
                page_no,
                success,
                completed_pages,
                total_pages,
            } => Some((page_no, success, completed_pages, total_pages)),
            _ => None,
        })
        .collect()
}

/// The docling sequence: started first, completed last with `status`, every
/// event about document `index` named `name`.
fn assert_brackets(events: &[ProgressEvent], index: usize, name: &str, status: ConversionStatus) {
    assert!(!events.is_empty(), "no events");
    assert!(
        events
            .iter()
            .all(|e| e.document_index == index && e.document_name == name),
        "{events:?}"
    );
    assert_eq!(events[0].kind, ProgressKind::DocumentStarted, "{events:?}");
    assert_eq!(
        events[events.len() - 1].kind,
        ProgressKind::DocumentCompleted { status },
        "{events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, ProgressKind::DocumentStarted))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, ProgressKind::DocumentCompleted { .. }))
            .count(),
        1
    );
}

/// The issue's first case: a 3-page PDF with `pages=2-3` yields a start
/// event, two page events reaching 2/2 and a completion event. The page
/// numbers are the source document's.
#[test]
fn a_page_window_reports_its_pages_and_reaches_the_total() {
    let (callback, seen) = recorder();
    let result = DocumentConverter::new()
        .text_layer_only(true)
        .page_range(2, 3)
        .progress_callback(callback)
        .convert(three_pages())
        .expect("convert");
    assert_eq!(result.status, ConversionStatus::Success);
    let events = seen.lock().unwrap().clone();
    assert_brackets(&events, 1, "skipped_1page", ConversionStatus::Success);
    let pages = pages(&events);
    assert_eq!(pages.len(), 2, "{events:?}");
    // Pages may finish out of order on the parallel walk; the count climbs.
    assert_eq!(
        pages.iter().map(|p| (p.1, p.2, p.3)).collect::<Vec<_>>(),
        vec![(true, 1, 2), (true, 2, 2)]
    );
    let mut numbers: Vec<usize> = pages.iter().map(|p| p.0).collect();
    numbers.sort_unstable();
    assert_eq!(numbers, vec![2, 3]);
}

/// The issue's second case: with a tiny `document_timeout` the pages the
/// budget cut off arrive with `success=false`, in page order, and the count
/// still reaches the total; the document completes as a partial success.
#[test]
fn cut_off_pages_are_reported_as_failed_so_the_count_reaches_the_total() {
    let (callback, seen) = recorder();
    let result = DocumentConverter::new()
        .text_layer_only(true)
        .document_timeout(Some(Duration::from_nanos(1)))
        .progress_callback(callback)
        .convert(three_pages())
        .expect("a timeout is not a failure");
    assert_eq!(result.status, ConversionStatus::PartialSuccess);
    let events = seen.lock().unwrap().clone();
    assert_brackets(
        &events,
        1,
        "skipped_1page",
        ConversionStatus::PartialSuccess,
    );
    assert_eq!(
        pages(&events),
        vec![(1, false, 1, 3), (2, false, 2, 3), (3, false, 3, 3)]
    );
}

/// A window plus a spent budget: only the window's pages are failed, with
/// the window's numbering and total.
#[test]
fn cut_off_pages_keep_the_window_numbering() {
    let (callback, seen) = recorder();
    DocumentConverter::new()
        .text_layer_only(true)
        .page_range(2, 3)
        .document_timeout(Some(Duration::from_nanos(1)))
        .progress_callback(callback)
        .convert(three_pages())
        .expect("convert");
    let events = seen.lock().unwrap().clone();
    assert_eq!(pages(&events), vec![(2, false, 1, 2), (3, false, 2, 2)]);
}

/// The streaming conversion reports the same sequence, the completion
/// after the last chunk.
#[test]
fn the_streaming_conversion_reports_the_same_events() {
    let (callback, seen) = recorder();
    let stream = DocumentConverter::new()
        .text_layer_only(true)
        .page_range(2, 3)
        .progress_callback(callback)
        .convert_streaming(three_pages())
        .expect("stream starts");
    let chunks: Vec<_> = stream.collect();
    assert!(chunks.iter().all(|c| c.is_ok()), "{chunks:?}");
    let events = seen.lock().unwrap().clone();
    assert_brackets(&events, 1, "skipped_1page", ConversionStatus::Success);
    assert_eq!(
        pages(&events)
            .iter()
            .map(|p| (p.1, p.2, p.3))
            .collect::<Vec<_>>(),
        vec![(true, 1, 2), (true, 2, 2)]
    );

    // And a spent budget on the streaming path: failed pages, partial.
    let (callback, seen) = recorder();
    let stream = DocumentConverter::new()
        .text_layer_only(true)
        .document_timeout(Some(Duration::from_nanos(1)))
        .progress_callback(callback)
        .convert_streaming(three_pages())
        .expect("stream starts");
    let _ = stream.collect::<Vec<_>>();
    let events = seen.lock().unwrap().clone();
    assert_brackets(
        &events,
        1,
        "skipped_1page",
        ConversionStatus::PartialSuccess,
    );
    assert_eq!(
        pages(&events),
        vec![(1, false, 1, 3), (2, false, 2, 3), (3, false, 3, 3)]
    );
}

/// `convert_with_progress`: the document index is the caller's, the
/// call's callback and the converter's both get every event. A format
/// without pages reports the start and the end only; a failed conversion
/// ends with `failure`.
#[test]
fn per_call_callback_index_and_the_declarative_and_failed_cases() {
    let (on_converter, seen_converter) = recorder();
    let (on_call, seen_call) = recorder();
    let converter = DocumentConverter::new().progress_callback(on_converter);
    let md = SourceDocument::from_bytes("notes.md", docling::InputFormat::Md, b"# Hi\n".to_vec());
    converter
        .convert_with_progress(md, 7, Some(on_call))
        .expect("convert");
    for seen in [&seen_converter, &seen_call] {
        let events = seen.lock().unwrap().clone();
        assert_brackets(&events, 7, "notes.md", ConversionStatus::Success);
        assert_eq!(events.len(), 2, "{events:?}");
    }

    let (callback, seen) = recorder();
    let broken = SourceDocument::from_bytes(
        "broken.docx",
        docling::InputFormat::Docx,
        b"not a zip".to_vec(),
    );
    let err = DocumentConverter::new()
        .progress_callback(callback)
        .convert(broken);
    assert!(err.is_err());
    let events = seen.lock().unwrap().clone();
    assert_brackets(&events, 1, "broken.docx", ConversionStatus::Failure);
}

/// A callback that panics does not fail the conversion.
#[test]
fn a_panicking_callback_is_contained() {
    let bad: ProgressCallback = Arc::new(|e| {
        if matches!(e.kind, ProgressKind::PageCompleted { .. }) {
            panic!("observer bug");
        }
    });
    let result = DocumentConverter::new()
        .text_layer_only(true)
        .progress_callback(bad)
        .convert(three_pages())
        .expect("the conversion survives its observer");
    assert_eq!(result.status, ConversionStatus::Success);
}

/// The real pipeline (layout, tables, OCR) reports the same sequence —
/// the issue's first case end to end. Skips without the models.
#[test]
fn the_ml_pipeline_reports_its_pages() {
    if !ml_stack_ready() {
        eprintln!("skipping: ML models not present");
        return;
    }
    let (callback, seen) = recorder();
    let result = DocumentConverter::new()
        .page_range(2, 3)
        .progress_callback(callback)
        .convert(three_pages())
        .expect("convert");
    assert_eq!(result.status, ConversionStatus::Success);
    let events = seen.lock().unwrap().clone();
    assert_brackets(&events, 1, "skipped_1page", ConversionStatus::Success);
    assert_eq!(
        pages(&events)
            .iter()
            .map(|p| (p.1, p.2, p.3))
            .collect::<Vec<_>>(),
        vec![(true, 1, 2), (true, 2, 2)]
    );
}
