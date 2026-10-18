//! Pluggable backends (#680): a backend registered on a converter claims
//! sources by extension, media type or content, ahead of the built-in
//! formats, in priority order; the converter's loaders accept the formats
//! only a registered backend knows; the document goes through the usual
//! post-processing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use docling::backend::DeclarativeBackend;
use docling::{BackendMatcher, ConversionError, DocumentConverter, InputFormat, SourceDocument};
use docling_core::{DoclingDocument, Node};

/// A backend that writes one paragraph naming itself and what it read.
struct Tag(&'static str);

impl DeclarativeBackend for Tag {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let mut doc = DoclingDocument::new(&source.name);
        doc.push(Node::Paragraph {
            text: format!("{} read {} bytes", self.0, source.bytes.len()),
        });
        Ok(doc)
    }
}

/// A backend that always fails.
struct Broken;

impl DeclarativeBackend for Broken {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        Err(ConversionError::Parse(format!(
            "broken backend on {}",
            source.name
        )))
    }
}

fn md(converter: &DocumentConverter, source: SourceDocument) -> String {
    converter
        .convert(source)
        .expect("converts")
        .document
        .export_to_markdown()
}

fn scratch(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("docling-plugins-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

const BLOB: &[u8] = b"\x00XYZ\x01\x02 binary payload \xff\xfe";

/// An extension no table knows, holding bytes that are not text: unknown to
/// `SourceDocument::from_file` and to a plain converter, the format a
/// registered backend claims.
#[test]
fn an_unknown_binary_format_loads_for_its_backend() {
    let path = scratch("data.xyz", BLOB);
    assert!(matches!(
        SourceDocument::from_file(&path),
        Err(ConversionError::UnknownFormat { .. })
    ));
    assert!(matches!(
        DocumentConverter::new().load_file(&path),
        Err(ConversionError::UnknownFormat { .. })
    ));

    let converter = DocumentConverter::new().register_backend(
        "xyz",
        BackendMatcher::new().extensions(["xyz"]),
        Arc::new(Tag("xyz")),
        0,
    );
    let source = converter.load_file(&path).unwrap();
    assert_eq!(source.format, InputFormat::Plugin);
    assert_eq!(source.extension.as_deref(), Some("xyz"));
    let result = converter.convert(source).unwrap();
    assert_eq!(result.format, InputFormat::Plugin);
    assert_eq!(result.input_name, "data");
    assert_eq!(
        result.document.export_to_markdown(),
        format!("xyz read {} bytes\n", BLOB.len())
    );

    // By content: a nameless upload, the media type, both reach it.
    let sniffing = DocumentConverter::new().register_backend(
        "xyz",
        BackendMatcher::new()
            .media_types(["application/x-xyz"])
            .sniff(|b| b.starts_with(b"\x00XYZ")),
        Arc::new(Tag("xyz")),
        0,
    );
    for (name, media) in [("blob", None), ("upload", Some("application/x-xyz"))] {
        let source = sniffing
            .load_named_bytes(name, BLOB.to_vec(), media)
            .unwrap();
        assert_eq!(source.format, InputFormat::Plugin, "{name}");
        assert!(md(&sniffing, source).starts_with("xyz read"), "{name}");
    }
    // The media type survives loading, so the match holds at conversion.
    let source = DocumentConverter::new()
        .register_backend(
            "xyz",
            BackendMatcher::new().media_types(["application/x-xyz"]),
            Arc::new(Tag("xyz")),
            0,
        )
        .load_named_bytes("upload", BLOB.to_vec(), Some("application/x-xyz; v=1"))
        .unwrap();
    assert_eq!(source.media_type.as_deref(), Some("application/x-xyz; v=1"));

    // Loaded for a backend this converter lacks: a clear error, not a guess.
    let err = DocumentConverter::new()
        .convert(converter.load_file(&path).unwrap())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("none registered on this converter claims it"),
        "{err}"
    );
}

/// A registered backend comes before the built-in format: a notebook is
/// the backend's, and keeps its built-in format id.
#[test]
fn a_backend_takes_over_a_built_in_format() {
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ipynb/sources/milk_quiz.ipynb");
    let plain = DocumentConverter::new();
    let built_in = md(&plain, plain.load_file(&notebook).unwrap());
    assert!(!built_in.contains("notebooks read"), "{built_in}");

    let converter = DocumentConverter::new().register_backend(
        "notebooks",
        BackendMatcher::new().extensions([".IPYNB"]),
        Arc::new(Tag("notebooks")),
        0,
    );
    let source = converter.load_file(&notebook).unwrap();
    assert_eq!(source.format, InputFormat::Ipynb);
    let result = converter.convert(source).unwrap();
    assert_eq!(result.format, InputFormat::Ipynb);
    assert!(result
        .document
        .export_to_markdown()
        .starts_with("notebooks read"));
    // SourceDocument::from_file's sources are claimed at conversion too.
    let source = SourceDocument::from_file(&notebook).unwrap();
    assert!(md(&converter, source).starts_with("notebooks read"));
}

/// Highest priority first; among equals, the earliest registration.
#[test]
fn priority_orders_the_registered_backends() {
    let claim = || BackendMatcher::new().extensions(["x"]);
    let converter = DocumentConverter::new()
        .register_backend("first", claim(), Arc::new(Tag("first")), 0)
        .register_backend("second", claim(), Arc::new(Tag("second")), 0)
        .register_backend("urgent", claim(), Arc::new(Tag("urgent")), 10)
        .register_backend("late", claim(), Arc::new(Tag("late")), -5);
    let names: Vec<&str> = converter
        .registered_backends()
        .iter()
        .map(|b| b.name())
        .collect();
    assert_eq!(names, ["urgent", "first", "second", "late"]);
    let source = converter
        .load_named_bytes("a.x", b"x".to_vec(), None)
        .unwrap();
    assert!(md(&converter, source).starts_with("urgent read"));

    let tied = DocumentConverter::new()
        .register_backend("first", claim(), Arc::new(Tag("first")), 0)
        .register_backend("second", claim(), Arc::new(Tag("second")), 0);
    let source = tied.load_named_bytes("a.x", b"x".to_vec(), None).unwrap();
    assert!(md(&tied, source).starts_with("first read"));
}

/// A registered backend's failure is the conversion's: no retry as the
/// content's built-in format.
#[test]
fn a_failing_backend_is_not_retried_as_a_built_in_format() {
    let converter = DocumentConverter::new().register_backend(
        "broken",
        BackendMatcher::new().extensions(["md"]),
        Arc::new(Broken),
        0,
    );
    let source = converter
        .load_named_bytes("notes.md", b"# Title\n\ntext\n".to_vec(), None)
        .unwrap();
    let err = converter.convert(source).unwrap_err().to_string();
    assert!(err.contains("broken backend on notes"), "{err}");
}

/// The backend's document gets the converter's post-processing — here the
/// PII redaction pass — and the streaming path converts a claimed PDF
/// through the backend, not the page pipeline.
#[test]
fn backend_documents_get_the_usual_post_processing_and_streaming() {
    struct Email;
    impl DeclarativeBackend for Email {
        fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
            let mut doc = DoclingDocument::new(&source.name);
            doc.push(Node::Paragraph {
                text: "write to jane.doe@example.com".into(),
            });
            Ok(doc)
        }
    }
    let converter = DocumentConverter::new()
        .register_backend(
            "email",
            BackendMatcher::new().extensions(["mail2"]),
            Arc::new(Email),
            0,
        )
        .redact_pii(docling_core::RedactionOptions::default());
    let source = converter
        .load_named_bytes("m.mail2", b"x".to_vec(), None)
        .unwrap();
    let result = converter.convert(source).unwrap();
    let text = result.document.export_to_markdown();
    assert!(!text.contains("jane.doe@example.com"), "{text}");
    assert!(result.redaction.is_some());

    let pdf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/pdf/sources/2206.01062.pdf");
    let streaming = DocumentConverter::new().register_backend(
        "pdf-override",
        BackendMatcher::new().sniff(|b| b.starts_with(b"%PDF")),
        Arc::new(Tag("pdf-override")),
        0,
    );
    let chunks: Vec<String> = streaming
        .convert_streaming(streaming.load_file(&pdf).unwrap())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        chunks.concat().starts_with("pdf-override read"),
        "{chunks:?}"
    );
}

/// Listing: the registered extensions join the built-in ones; a converter
/// without registrations lists exactly the build's formats.
#[test]
fn supported_extensions_include_registered_backends() {
    let plain = DocumentConverter::new().supported_extensions();
    let built_in: Vec<String> = InputFormat::supported_extensions()
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(plain, built_in);
    assert!(!plain.contains(&"xyz".to_string()));

    let converter = DocumentConverter::new().register_backend(
        "xyz",
        BackendMatcher::new().extensions(["xyz", "md"]),
        Arc::new(Tag("xyz")),
        0,
    );
    let listed = converter.supported_extensions();
    assert!(listed.contains(&"xyz".to_string()));
    assert_eq!(listed.iter().filter(|e| *e == "md").count(), 1);
    assert_eq!(listed.len(), plain.len() + 1);
    assert_eq!(InputFormat::from_id("plugin"), Some(InputFormat::Plugin));
    assert_eq!(InputFormat::Plugin.as_str(), "plugin");
}
