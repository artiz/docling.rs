//! `--to vtt` (#614): the WebVTT export against docling 2.135's own output.
//!
//! `tests/data/webvtt/vtt_out/` holds what Python docling writes for the
//! four mirrored WebVTT inputs (`docling-core` 2.101's `WebVTTDocSerializer`):
//! `<stem>.cli.vtt` is `save_as_vtt` — docling's `--to vtt`, the voice end
//! tag of a lone voice span omitted — and `<stem>.export.vtt` is
//! `export_to_vtt()` with its defaults (every end tag kept).

use std::path::Path;

use docling::{DocumentConverter, SourceDocument};
use docling_core::VttExportOptions;

#[test]
fn webvtt_round_trips_like_docling() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let want_dir = root.join("tests/data/webvtt/vtt_out");
    for n in 1..=4 {
        let stem = format!("webvtt_example_0{n}");
        let src = root.join(format!("../../tests/data/webvtt/sources/{stem}.vtt"));
        let doc = DocumentConverter::new()
            .convert(SourceDocument::from_file(&src).unwrap())
            .unwrap()
            .document;
        let cli = std::fs::read_to_string(want_dir.join(format!("{stem}.cli.vtt"))).unwrap();
        assert_eq!(doc.export_to_vtt(), cli, "{stem} (--to vtt)");
        let export = std::fs::read_to_string(want_dir.join(format!("{stem}.export.vtt"))).unwrap();
        let options = VttExportOptions {
            omit_hours_if_zero: false,
            omit_voice_end: false,
        };
        assert_eq!(
            doc.export_to_vtt_with_options(&options),
            export,
            "{stem} (export_to_vtt)"
        );
    }
}

/// A `.vtt` round-trips: the exported file converts back to the same cues.
#[test]
fn exported_webvtt_converts_back_to_itself() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for n in 1..=4 {
        let src = root.join(format!(
            "../../tests/data/webvtt/sources/webvtt_example_0{n}.vtt"
        ));
        let first = DocumentConverter::new()
            .convert(SourceDocument::from_file(&src).unwrap())
            .unwrap()
            .document
            .export_to_vtt();
        let again = DocumentConverter::new()
            .convert(SourceDocument::from_bytes(
                "again.vtt",
                docling::InputFormat::Vtt,
                first.clone().into_bytes(),
            ))
            .unwrap()
            .document
            .export_to_vtt();
        assert_eq!(again, first, "webvtt_example_0{n}");
    }
}

/// A document without timed text is the bare header, titled by its title
/// item — what docling 2.135 writes for these DOCX files (`export_to_vtt`
/// on upstream's groundtruth JSON).
#[test]
fn untimed_documents_write_the_header_only() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (name, want) in [
        ("word_sample.docx", "WEBVTT Swimming in the lake"),
        ("unit_test_headers.docx", "WEBVTT Test Document"),
        ("lorem_ipsum.docx", "WEBVTT"),
    ] {
        let src = root.join(format!("../../tests/data/docx/sources/{name}"));
        let doc = DocumentConverter::new()
            .convert(SourceDocument::from_file(&src).unwrap())
            .unwrap()
            .document;
        assert_eq!(doc.export_to_vtt(), want, "{name}");
    }
}
