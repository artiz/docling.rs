//! Document metadata (#679): the file's own — an EPUB's Dublin Core, an
//! Office package's properties, an ODF `meta.xml`, a photo's EXIF, an
//! audio file's tags, a PDF's Info dictionary — read on request into the
//! document, written as the JSON's `metadata` object or as YAML front
//! matter; nothing by default; covered by the PII pass.

use std::path::{Path, PathBuf};

use docling::{ConvertOptions, DocumentConverter, MetadataOutput, SourceDocument};
use docling_core::{DoclingDocument, RedactionOptions};

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const EPUB: &str = "crates/docling-py/tests/data/markitdown/test.epub";
const DOCX: &str = "crates/docling-py/tests/data/markitdown/test.docx";
const JPEG: &str = "crates/docling/tests/data/metadata/geotagged.jpg";

fn source(rel: &str) -> SourceDocument {
    SourceDocument::from_file(repo(rel)).expect("fixture")
}

/// The issue's acceptance: `test.epub` with front matter shows
/// `authors: [Test Author]`; the default output is unchanged.
#[test]
fn epub_front_matter_names_the_author_and_the_default_is_unchanged() {
    let plain = DocumentConverter::new()
        .convert(source(EPUB))
        .unwrap()
        .document;
    assert!(plain.metadata.is_empty());
    let md = plain.export_to_markdown();
    assert!(!md.starts_with("---"), "{md}");
    assert!(!plain.export_to_json().contains("\"metadata\""));

    let doc = DocumentConverter::new()
        .metadata_output(MetadataOutput::Frontmatter)
        .convert(source(EPUB))
        .unwrap()
        .document;
    let with = doc.export_to_markdown();
    assert!(
        with.starts_with(
            "---\n\
             title: Test EPUB Document\n\
             authors: [Test Author]\n\
             description: A test EPUB document for MarkItDown testing\n\
             language: en\n\
             identifier: test-book-id\n\
             modified: \"2024-12-18T10:53:14Z\"\n\
             creator_tool: Ebook-lib 0.18.1\n\
             ---\n\n"
        ),
        "{with}"
    );
    // The body is the plain export, after the block.
    assert_eq!(&with[with.find("---\n\n").unwrap() + 5..], md);
    // The JSON carries the object after `origin`; the plain-text export
    // never carries the front matter.
    let json = doc.export_to_json();
    let at_origin = json.find("\"origin\"").unwrap();
    let at_meta = json.find("\"metadata\": {").expect("metadata object");
    assert!(at_meta > at_origin && at_meta < json.find("\"furniture\"").unwrap());
    assert!(
        json.contains("\"authors\": [\n      \"Test Author\"\n    ]"),
        "{json}"
    );
    assert!(!doc.export_to_text().starts_with("---"));

    // `json` mode: the object, no front matter.
    let doc = DocumentConverter::new()
        .metadata_output(MetadataOutput::Json)
        .convert(source(EPUB))
        .unwrap()
        .document;
    assert!(doc.export_to_json().contains("\"metadata\": {"));
    assert_eq!(doc.export_to_markdown(), md);
}

/// The acceptance's JPEG: `DateTimeOriginal` (and the rest of the EXIF,
/// the decoded GPS position, the Windows `XP*` tags) in the front matter.
#[test]
fn jpeg_exif_reaches_the_front_matter() {
    let m = DocumentConverter::new().document_metadata(&source(JPEG));
    assert_eq!(m.exif["DateTimeOriginal"], "2024-01-02 03:04:05", "{m:?}");
    assert_eq!(m.exif["ImageDescription"], "A red test card");
    assert_eq!(m.exif["Software"], "docling.rs tests");
    assert_eq!(m.exif["XPTitle"], "Test card");
    assert_eq!(m.exif["ExposureTime"], "1/125 s");
    assert!(m.exif.contains_key("GPSLatitude"), "{m:?}");
    assert_eq!(m.title.as_deref(), Some("Test card"));
    assert_eq!(m.description.as_deref(), Some("A red test card"));
    assert_eq!(m.authors, ["Ada Lovelace", "Charles Babbage"]);
    assert_eq!(m.keywords, ["red", "card"]);
    assert_eq!(m.date.as_deref(), Some("2024-01-02T03:04:05"));
    assert_eq!(m.created.as_deref(), Some("2024-01-02T03:04:06"));
    assert_eq!(m.rights.as_deref(), Some("CC0"));
    assert_eq!(m.properties["image_size"], "8x6");
    let gps = m.gps.as_ref().expect("gps");
    assert!((gps.latitude - 48.8566).abs() < 1e-4 && (gps.longitude - 2.3522).abs() < 1e-4);
    assert_eq!(gps.altitude, Some(35.0));

    let mut doc = DoclingDocument::new("geotagged");
    doc.metadata = m;
    doc.metadata_output = MetadataOutput::Frontmatter;
    let md = doc.export_to_markdown();
    assert!(
        md.starts_with("---\ntitle: Test card\nauthors: [Ada Lovelace, Charles Babbage]\n"),
        "{md}"
    );
    assert!(md.contains("\nexif:\n"), "{md}");
    assert!(
        md.contains("\n  DateTimeOriginal: \"2024-01-02 03:04:05\"\n"),
        "{md}"
    );
    assert!(
        md.contains("\ngps: {latitude: 48.8566, longitude: 2.3522, altitude: 35}\n"),
        "{md}"
    );
    assert!(md.ends_with("---\n"), "{md}");
    // A file without EXIF: the size alone.
    let png = SourceDocument::from_bytes(
        "p.png",
        docling::InputFormat::Image,
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52,
            0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49,
            0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xFF, 0xFF, 0x3F, 0, 5, 0xFE, 2, 0xFE, 0xA3,
            0x4A, 0x4E, 0x1C, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ],
    );
    let m = DocumentConverter::new().document_metadata(&png);
    assert_eq!(m.properties["image_size"], "1x1");
    assert!(m.exif.is_empty() && m.gps.is_none());
}

/// Office core/app properties, an ODF `meta.xml`, a PDF's Info dictionary
/// and an audio file's tags, through the converter.
#[test]
fn office_odf_pdf_and_audio_carry_their_properties() {
    let doc = DocumentConverter::new()
        .metadata_output(MetadataOutput::Json)
        .convert(source(DOCX))
        .unwrap()
        .document;
    let m = &doc.metadata;
    assert_eq!(m.authors, ["Adam Fourney"], "{m:?}");
    assert!(m
        .last_modified_by
        .as_deref()
        .is_some_and(|b| b.starts_with("Yuzhong Zhang")));
    assert_eq!(m.creator_tool.as_deref(), Some("Microsoft Office Word"));
    assert_eq!(m.properties["pages"], "2");
    assert_eq!(m.properties["words"], "664");
    assert_eq!(m.properties["revision"], "3");
    assert!(
        m.created.as_deref().is_some_and(|c| c.starts_with("20")),
        "{m:?}"
    );
    assert!(m.title.is_none(), "an empty <dc:title/> is no title: {m:?}");

    let odp = DocumentConverter::new()
        .document_metadata(&source("tests/data/odf/sources/odf_presentation_01.odp"));
    assert!(
        odp.creator_tool
            .as_deref()
            .is_some_and(|g| g.starts_with("LibreOffice")),
        "{odp:?}"
    );

    #[cfg(any(feature = "pdf", feature = "pdf-text"))]
    {
        let pdf = DocumentConverter::new()
            .document_metadata(&source("tests/data/pdf/sources/2203.01017v2.pdf"));
        assert_eq!(
            pdf.created.as_deref(),
            Some("2022-03-14T01:27:27Z"),
            "{pdf:?}"
        );
        assert!(pdf.creator_tool.is_some(), "{pdf:?}");
    }
    #[cfg(feature = "asr")]
    {
        let mp3 = DocumentConverter::new()
            .document_metadata(&source("tests/data/audio/sources/sample_10s.mp3"));
        assert_eq!(
            mp3.title.as_deref(),
            Some("Shakespeare on Scenery"),
            "{mp3:?}"
        );
        assert_eq!(mp3.authors, ["Oscar Wilde"]);
        assert_eq!(
            mp3.audio["album"],
            "LibriVox Nonfiction Collection Vol. 023"
        );
        assert_eq!(mp3.audio["genre"], "Speech");
        assert!(
            mp3.audio.contains_key("sample_rate") && mp3.audio.contains_key("duration"),
            "{mp3:?}"
        );
        let flac = DocumentConverter::new().document_metadata(&source(
            "tests/data/audio/sources/sample_10s_audio-flac.flac",
        ));
        assert_eq!(flac.authors, ["Oscar Wilde"], "{flac:?}");
    }
    // A format without any: empty.
    let csv =
        SourceDocument::from_bytes("a.csv", docling::InputFormat::Csv, b"a,b\n1,2\n".to_vec());
    assert!(DocumentConverter::new().document_metadata(&csv).is_empty());
}

/// The PII pass masks the author fields as persons and drops the GPS
/// position; the rest goes through the detectors.
#[test]
fn redaction_masks_authors_and_drops_the_position() {
    let r = DocumentConverter::new()
        .metadata_output(MetadataOutput::Frontmatter)
        .redact_pii(RedactionOptions::default())
        .convert(source(EPUB))
        .unwrap();
    assert_eq!(r.document.metadata.authors, ["[PERSON]"]);
    assert_eq!(
        r.document.metadata.title.as_deref(),
        Some("Test EPUB Document")
    );
    assert!(r.redaction.unwrap().counts["PERSON"] >= 1);
    assert!(r
        .document
        .export_to_markdown()
        // A placeholder opens with `[`, so the list goes block style, quoted.
        .contains("authors:\n  - \"[PERSON]\"\n"));

    let mut doc = DoclingDocument::new("geotagged");
    doc.metadata = DocumentConverter::new().document_metadata(&source(JPEG));
    doc.metadata.description = Some("Mail ada@example.com".into());
    let opts = RedactionOptions::default();
    let detector = docling_core::redact::PatternDetector::new(&opts).unwrap();
    let report = doc.redact(&opts, &detector);
    let m = &doc.metadata;
    assert!(m.gps.is_none());
    assert!(!m.exif.keys().any(|k| k.starts_with("GPS")), "{m:?}");
    assert_eq!(m.authors, ["[PERSON]", "[PERSON]"]);
    assert_eq!(m.exif["Artist"], "[PERSON]");
    assert_eq!(m.description.as_deref(), Some("Mail [EMAIL]"));
    assert_eq!(m.exif["ImageDescription"], "A red test card");
    assert_eq!(report.counts["LOCATION"], 1);
    assert_eq!(report.counts["PERSON"], 3);
    assert_eq!(report.counts["EMAIL"], 1);

    // `kinds=email` leaves the names and the position alone.
    let mut doc = DoclingDocument::new("geotagged");
    doc.metadata = DocumentConverter::new().document_metadata(&source(JPEG));
    let opts = RedactionOptions {
        kinds: vec![docling_core::PiiKind::Email],
        ..Default::default()
    };
    let detector = docling_core::redact::PatternDetector::new(&opts).unwrap();
    doc.redact(&opts, &detector);
    assert!(doc.metadata.gps.is_some());
    assert_eq!(doc.metadata.authors, ["Ada Lovelace", "Charles Babbage"]);
}

/// The JSON round trip keeps the object; the streaming export opens with
/// the same front matter as the buffered one; the option's spelling is
/// validated with the others.
#[test]
fn json_round_trip_streaming_and_the_option() {
    let json = DocumentConverter::new()
        .metadata_output(MetadataOutput::Json)
        .convert(source(EPUB))
        .unwrap()
        .document
        .export_to_json();
    let back = DocumentConverter::new()
        .metadata_output(MetadataOutput::Frontmatter)
        .convert(SourceDocument::from_bytes(
            "test.json",
            docling::InputFormat::JsonDocling,
            json.clone().into_bytes(),
        ))
        .unwrap()
        .document;
    assert_eq!(back.metadata.authors, ["Test Author"]);
    assert!(back
        .export_to_markdown()
        .starts_with("---\ntitle: Test EPUB Document\n"));
    // Without a mode the object is kept but not written.
    let quiet = DocumentConverter::new()
        .convert(SourceDocument::from_bytes(
            "test.json",
            docling::InputFormat::JsonDocling,
            json.into_bytes(),
        ))
        .unwrap()
        .document;
    assert_eq!(quiet.metadata.authors, ["Test Author"]);
    assert!(!quiet.export_to_json().contains("\"metadata\""));

    let converter = DocumentConverter::new().metadata_output(MetadataOutput::Frontmatter);
    let buffered = converter
        .convert(source(EPUB))
        .unwrap()
        .document
        .export_to_markdown();
    let streamed: String = converter
        .convert_streaming(source(EPUB))
        .unwrap()
        .map(|c| c.unwrap())
        .collect();
    assert_eq!(streamed, buffered);

    let bad = ConvertOptions {
        metadata: Some("yaml".into()),
        ..Default::default()
    };
    assert_eq!(
        bad.validate().unwrap_err().cli_message(),
        "--metadata must be one of none, json, frontmatter; got \"yaml\""
    );
    let ok = ConvertOptions {
        metadata: Some("Front-Matter".into()),
        ..Default::default()
    };
    assert_eq!(
        ok.metadata_output().unwrap(),
        Some(MetadataOutput::Frontmatter)
    );
    assert_eq!(ConvertOptions::default().metadata_output().unwrap(), None);
}
