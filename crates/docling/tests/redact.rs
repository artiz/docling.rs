//! e2e for #621: the opt-in PII redaction pass over every format's model.
//! Each `pii_sample.*` fixture seeds the same values — an e-mail, a phone
//! number, a Luhn-valid card, an SSN, an IBAN and a name — in body text, a
//! table, a DOCX comment, an HTML `mailto:` link and figure caption, an
//! e-mail's headers, a VTT voice tag, a PDF text layer and a scanned PDF's
//! pixels; with the pass on, no export (Markdown, JSON, DocLang, text,
//! chunks) carries any of them, the JSON of the tree-building backends
//! (HTML, DOCX) included. Pseudonyms are consistent, the streaming Markdown
//! is byte-identical to the buffered one, the image modes behave, and a
//! missing NER model is a warning. The ML-dependent cases skip without the
//! models, like the other ML tests.

use std::path::{Path, PathBuf};

use docling::{
    DocumentConverter, ImageRedaction, PiiKind, RedactionOptions, Replacement, SourceDocument,
};

const EMAIL: &str = "john.doe@example.com";
const PHONE: &str = "555) 123-4567";
const CARD: &str = "4111 1111 1111 1111";
const SSN: &str = "123-45-6789";
const IBAN: &str = "DE89 3704 0044 0532 0130 00";
const SEEDED: [&str; 5] = [EMAIL, PHONE, CARD, SSN, IBAN];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn data(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(rel)
}

fn ml_stack_ready() -> bool {
    let models = repo_root().join(".models");
    models.join("layout_heron.onnx").exists()
        && (models.join("ocr_rec_v6.onnx").exists() || models.join("ocr_rec_en.onnx").exists())
        && std::env::set_current_dir(repo_root()).is_ok()
}

fn redacting() -> DocumentConverter {
    DocumentConverter::new().redact_pii(RedactionOptions::default())
}

fn convert(converter: DocumentConverter, rel: &str) -> docling::ConversionResult {
    let source = SourceDocument::from_file(data(rel)).expect("fixture");
    converter.convert(source).expect("convert")
}

/// Every export of the document, named.
fn exports(doc: &docling_core::DoclingDocument) -> Vec<(&'static str, String)> {
    let chunks = docling_core::chunker::HierarchicalChunker
        .chunk(doc)
        .into_iter()
        .map(|c| c.text)
        .collect::<Vec<_>>()
        .join("\n");
    vec![
        ("markdown", doc.export_to_markdown()),
        ("strict markdown", doc.export_to_markdown_with(true)),
        ("json", doc.export_to_json()),
        ("doclang", doc.export_to_doclang()),
        ("text", doc.export_to_text()),
        ("chunks", chunks),
    ]
}

fn assert_clean(doc: &docling_core::DoclingDocument, what: &str) {
    for (name, out) in exports(doc) {
        for seed in SEEDED {
            assert!(
                !out.contains(seed),
                "{what}: {name} export still carries {seed:?}:\n{out}"
            );
        }
    }
}

#[test]
fn declarative_formats_come_out_clean_with_counts() {
    for (rel, min_total) in [
        ("data/docx/sources/pii_sample.docx", 6),
        ("data/html/sources/pii_sample.html", 6),
        ("data/email/sources/pii_sample.eml", 5),
        ("data/webvtt/sources/pii_sample.vtt", 3),
    ] {
        let result = convert(redacting(), rel);
        assert_clean(&result.document, rel);
        let report = result.redaction.expect("the pass ran");
        assert!(
            report.total >= min_total,
            "{rel}: only {} redactions: {:?}",
            report.total,
            report.counts
        );
        assert!(report.mapping.is_none(), "no mapping unless asked");
        // The seeded values were all found.
        for label in ["EMAIL", "PHONE", "CREDIT_CARD", "NATIONAL_ID", "IBAN"] {
            if rel.ends_with(".vtt") && !["EMAIL", "PHONE", "CREDIT_CARD"].contains(&label) {
                continue;
            }
            assert!(
                report.counts.contains_key(label),
                "{rel}: no {label} in {:?}",
                report.counts
            );
        }
        // Off, the seeded values are there (the fixtures are real).
        let plain = convert(DocumentConverter::new(), rel);
        assert!(plain.redaction.is_none());
        assert!(plain.document.export_to_markdown().contains(EMAIL), "{rel}");
    }
}

/// The DOCX comment and the HTML `mailto:` / caption / alt go through the
/// item tree the JSON export reads; the Markdown link keeps its shape.
#[test]
fn tree_paths_and_links_are_redacted() {
    let docx = convert(redacting(), "data/docx/sources/pii_sample.docx");
    let json = docx.document.export_to_json();
    assert!(
        !json.contains("jane.reviewer@example.org"),
        "comment in the tree:\n{json}"
    );
    assert!(json.contains("[EMAIL]"));

    let html = convert(redacting(), "data/html/sources/pii_sample.html");
    let md = html.document.export_to_markdown();
    assert!(md.contains("[[EMAIL]](mailto:[EMAIL])"), "{md}");
    let json = html.document.export_to_json();
    assert!(!json.contains("mailto:john"), "{json}");
    assert!(json.contains("\"hyperlink\": \"mailto:[EMAIL]\""), "{json}");
}

#[test]
fn pseudonyms_are_consistent_and_fixed_text_is_literal() {
    let opts = RedactionOptions {
        replacement: Replacement::Pseudonym,
        return_mapping: true,
        ..Default::default()
    };
    let result = convert(
        DocumentConverter::new().redact_pii(opts),
        "data/docx/sources/pii_sample.docx",
    );
    let md = result.document.export_to_markdown();
    // The same e-mail in the body and the table, one number; the reviewer's
    // address in the comment is another (JSON only).
    assert_eq!(md.matches("[EMAIL_1]").count(), 2, "{md}");
    assert!(!md.contains("[EMAIL_2]"), "{md}");
    assert_eq!(md.matches("[PHONE_1]").count(), 2, "{md}");
    let json = result.document.export_to_json();
    assert!(json.contains("[EMAIL_2]"), "{json}");
    let map = result.redaction.unwrap().mapping.expect("asked for");
    assert!(
        map.iter().any(|(o, p)| o == EMAIL && p == "[EMAIL_1]"),
        "{map:?}"
    );

    let result = convert(
        DocumentConverter::new().redact_pii(RedactionOptions {
            replacement: Replacement::Fixed("█".into()),
            kinds: vec![PiiKind::Email],
            ..Default::default()
        }),
        "data/html/sources/pii_sample.html",
    );
    let md = result.document.export_to_markdown();
    assert!(md.contains("[█](mailto:█)"), "{md}");
    assert!(md.contains(PHONE), "other kinds untouched: {md}");
}

/// The streaming Markdown of a PDF, concatenated, is the buffered redacted
/// Markdown byte for byte — the pass runs per page batch with one numbering.
#[test]
fn streaming_matches_buffered_redacted_markdown() {
    if !ml_stack_ready() {
        eprintln!("skipping: models not found");
        return;
    }
    let opts = RedactionOptions {
        replacement: Replacement::Pseudonym,
        ..Default::default()
    };
    let converter = DocumentConverter::new().redact_pii(opts);
    let source = SourceDocument::from_file(data("fixtures/redact/pii_sample.pdf")).unwrap();
    let buffered = converter.clone().convert(source.clone()).unwrap();
    assert_clean(&buffered.document, "pdf text layer");
    let streamed: String = converter
        .convert_streaming(source)
        .expect("stream")
        .map(|c| c.expect("chunk"))
        .collect();
    assert_eq!(streamed, buffered.document.export_to_markdown());
    assert!(streamed.contains("[EMAIL_1]"), "{streamed}");
}

/// A scanned page: the OCR'd text is redacted like a text layer; `Drop`
/// removes the page image / picture payloads, `BoxOut` paints over the OCR
/// lines that carry a value (the repainted page image is no longer the
/// render), `Keep` leaves them.
#[test]
fn scanned_pdf_and_image_modes() {
    if !ml_stack_ready() {
        eprintln!("skipping: models not found");
        return;
    }
    let scan = "fixtures/redact/pii_sample_scan.pdf";
    let dropped = convert(
        DocumentConverter::new()
            .generate_page_images(true)
            .redact_pii(RedactionOptions::default()),
        scan,
    );
    assert_clean(&dropped.document, "scan");
    assert!(
        dropped.document.page_images.is_empty(),
        "Drop clears page images"
    );
    assert!(dropped.redaction.unwrap().counts.contains_key("EMAIL"));

    let kept = convert(
        DocumentConverter::new()
            .generate_page_images(true)
            .redact_pii(RedactionOptions {
                images: ImageRedaction::Keep,
                ..Default::default()
            }),
        scan,
    );
    assert_eq!(kept.document.page_images.len(), 1, "Keep leaves them");
    let kept_png = kept.document.page_images[&1].data.clone();

    let boxed = convert(
        DocumentConverter::new()
            .generate_page_images(true)
            .redact_pii(RedactionOptions {
                images: ImageRedaction::BoxOut,
                ..Default::default()
            }),
        scan,
    );
    assert_eq!(
        boxed.document.page_images.len(),
        1,
        "BoxOut keeps a repainted image"
    );
    let boxed_png = &boxed.document.page_images[&1].data;
    assert_ne!(
        boxed_png, &kept_png,
        "the lines with values were painted over"
    );
    // Reading the repainted page again finds no seeded value.
    let img = image::load_from_memory(boxed_png).unwrap().to_rgb8();
    let black = img.pixels().filter(|p| p.0 == [0, 0, 0]).count();
    assert!(black > 1000, "boxes painted: {black} black pixels");
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let reread = DocumentConverter::new()
        .convert(SourceDocument::from_bytes(
            "boxed.png",
            docling::InputFormat::Image,
            png,
        ))
        .unwrap();
    let md = reread.document.export_to_markdown();
    for seed in SEEDED {
        assert!(!md.contains(seed), "{seed} readable after box-out:\n{md}");
    }
}

/// A picture embedded in a DOCX (the same bitmap as the scan) goes through
/// the converter's box-out: `Drop` removes it, `Keep` leaves it, `BoxOut`
/// repaints it — and the repainted picture reads clean.
#[test]
fn embedded_picture_box_out() {
    if !ml_stack_ready() {
        eprintln!("skipping: models not found");
        return;
    }
    let rel = "data/docx/sources/pii_sample_picture.docx";
    let picture = |doc: &docling_core::DoclingDocument| -> Option<Vec<u8>> {
        doc.nodes.iter().find_map(|n| match n {
            docling_core::Node::Picture { image, .. } => {
                Some(image.as_ref().map(|i| i.data.clone()))
            }
            _ => None,
        })?
    };
    let plain = convert(DocumentConverter::new(), rel);
    let original = picture(&plain.document).expect("the DOCX embeds a picture");

    let dropped = convert(redacting(), rel);
    assert!(
        picture(&dropped.document).is_none(),
        "Drop removes the payload"
    );
    assert!(!dropped.document.export_to_json().contains("\"image\""));

    let kept = convert(
        DocumentConverter::new().redact_pii(RedactionOptions {
            images: ImageRedaction::Keep,
            ..Default::default()
        }),
        rel,
    );
    assert_eq!(
        picture(&kept.document).as_deref(),
        Some(original.as_slice())
    );

    let boxed = convert(
        DocumentConverter::new().redact_pii(RedactionOptions {
            images: ImageRedaction::BoxOut,
            ..Default::default()
        }),
        rel,
    );
    let png = picture(&boxed.document).expect("BoxOut keeps a repainted picture");
    assert_ne!(png, original);
    let reread = DocumentConverter::new()
        .convert(SourceDocument::from_bytes(
            "boxed.png",
            docling::InputFormat::Image,
            png,
        ))
        .unwrap();
    let md = reread.document.export_to_markdown();
    for seed in SEEDED {
        assert!(!md.contains(seed), "{seed} readable after box-out:\n{md}");
    }
    // The item tree's copy of the picture was repainted too.
    let json = boxed.document.export_to_json();
    assert!(json.contains("\"image\""), "{json}");
    assert!(!json.contains(&docling_core::base64::encode(&original)));
}

/// NER: with the model, the seeded name goes; without it, one warning and
/// the pattern kinds still redact (run in a child process so the model-dir
/// override never leaks into the other tests).
#[test]
fn ner_redacts_names_or_warns() {
    if std::env::var_os("DOCLING_RS_REDACT_CHILD").is_some() {
        let result = convert(redacting(), "data/html/sources/pii_sample.html");
        assert_clean(&result.document, "html without NER");
        assert!(result
            .document
            .export_to_markdown()
            .contains("Angela Merkel"));
        return;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["ner_redacts_names_or_warns", "--exact", "--nocapture"])
        .env("DOCLING_RS_REDACT_CHILD", "1")
        .env("DOCLING_RS_NER_DIR", "/nonexistent/ner")
        .output()
        .expect("spawn the child test");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "child failed:\n{stderr}");
    assert!(
        stderr.contains("NER model unavailable"),
        "no warning in:\n{stderr}"
    );

    if !repo_root().join(".models/ner/model.onnx").exists()
        || std::env::set_current_dir(repo_root()).is_err()
    {
        eprintln!("skipping the model half: NER model not found");
        return;
    }
    let result = convert(
        DocumentConverter::new().redact_pii(RedactionOptions {
            replacement: Replacement::Pseudonym,
            ..Default::default()
        }),
        "data/html/sources/pii_sample.html",
    );
    let md = result.document.export_to_markdown();
    assert!(!md.contains("Angela Merkel"), "{md}");
    assert!(md.contains("[PERSON_1]"), "{md}");
    assert!(result.redaction.unwrap().counts.contains_key("PERSON"));
}

/// An invalid custom pattern is the caller's error, not a silent skip.
#[test]
fn invalid_custom_pattern_is_an_error() {
    let opts = RedactionOptions {
        custom_patterns: vec![docling::CustomPattern {
            name: "bad".into(),
            regex: "(".into(),
        }],
        ..Default::default()
    };
    let source = SourceDocument::from_file(data("data/html/sources/pii_sample.html")).unwrap();
    let err = DocumentConverter::new()
        .redact_pii(opts)
        .convert(source)
        .expect_err("rejected");
    assert!(err.to_string().contains("redact pattern"), "{err}");
}
