//! Plain text, source code, generic JSON / JSON Lines and generic XML (#671):
//! the inputs MarkItDown's plain-text converter takes and docling.rs used to
//! fail on — or convert to an empty document.

use std::path::{Path, PathBuf};

use docling::{ConversionError, DocumentConverter, InputFormat, SourceDocument};
use docling_core::Node;

fn data(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/text/sources")
        .join(rel)
}

fn convert(rel: &str) -> docling::ConversionResult {
    let source = SourceDocument::from_file(data(rel)).expect("fixture");
    DocumentConverter::new().convert(source).expect("converts")
}

/// `(language, text)` of the document's single code block.
fn code_block(doc: &docling_core::DoclingDocument) -> (Option<String>, String) {
    let codes: Vec<_> = doc
        .nodes
        .iter()
        .filter_map(|n| match n {
            Node::Code { language, text, .. } => Some((language.clone(), text.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(codes.len(), 1, "one code block, got {:?}", doc.nodes);
    assert_eq!(
        doc.nodes.len(),
        1,
        "nothing but the code block: {:?}",
        doc.nodes
    );
    codes.into_iter().next().unwrap()
}

/// MarkItDown's `test.json`: a `.json` that is not a DoclingDocument is one
/// `json` code block holding the file verbatim — its UUID key included —
/// and no longer an empty document.
#[test]
fn generic_json_is_a_json_code_block() {
    let r = convert("test.json");
    assert_eq!(r.format, InputFormat::JsonDocling);
    let (lang, text) = code_block(&r.document);
    assert_eq!(lang.as_deref(), Some("json"));
    assert!(
        text.contains("5b64c88c-b3c3-4510-bcb8-da0b200602d8"),
        "{text}"
    );
    assert_eq!(
        text.trim(),
        std::fs::read_to_string(data("test.json")).unwrap().trim()
    );
    // docling's Markdown prints a bare fence; strict Markdown keeps the language.
    let md = r.document.export_to_markdown();
    assert!(md.starts_with("```\n{"), "{md}");
    assert!(md.contains("5b64c88c-b3c3-4510-bcb8-da0b200602d8"));
    assert!(md.trim_end().ends_with("```"), "{md}");
    assert!(r
        .document
        .export_to_markdown_with(true)
        .starts_with("```json\n{"));
}

/// `json_late_non_ascii.json`: the decoding keeps every character.
#[test]
fn non_ascii_json_keeps_its_characters() {
    let r = convert("json_late_non_ascii.json");
    let (_, text) = code_block(&r.document);
    for needle in ["café", "日本語", "😀"] {
        assert!(text.contains(needle), "{needle} lost");
        assert!(
            r.document.export_to_markdown().contains(needle),
            "{needle} lost in Markdown"
        );
    }
}

/// A DoclingDocument JSON still walks its body — the generic route only
/// takes what has no `schema_name` and no `body`.
#[test]
fn docling_json_is_still_parsed() {
    let json = r##"{"schema_name":"DoclingDocument","version":"1.7.0","name":"d",
        "body":{"self_ref":"#/body","children":[{"$ref":"#/texts/0"}],"name":"_root_","label":"unspecified"},
        "texts":[{"self_ref":"#/texts/0","parent":{"$ref":"#/body"},"children":[],"label":"paragraph","prov":[],"orig":"Hello","text":"Hello"}],
        "groups":[],"pictures":[],"tables":[],"key_value_items":[],"pages":{}}"##;
    let source = SourceDocument::from_bytes("d.json", InputFormat::JsonDocling, json.into());
    let doc = DocumentConverter::new().convert(source).unwrap().document;
    assert_eq!(doc.export_to_markdown().trim(), "Hello");
    assert!(!doc.nodes.iter().any(|n| matches!(n, Node::Code { .. })));

    // A broken DoclingDocument keeps its parse error; broken plain JSON
    // reads as text, as MarkItDown reads it.
    let broken = SourceDocument::from_bytes(
        "b.json",
        InputFormat::JsonDocling,
        br##"{"schema_name":"DoclingDocument", "body": "##.to_vec(),
    );
    assert!(DocumentConverter::new().convert(broken).is_err());
    let text =
        SourceDocument::from_bytes("t.json", InputFormat::JsonDocling, b"{not json".to_vec());
    let doc = DocumentConverter::new().convert(text).unwrap().document;
    assert_eq!(code_block(&doc).1, "{not json");
}

/// JSON Lines: one `json` block, every record on its own line, the
/// non-ASCII values intact.
#[test]
fn jsonl_is_a_json_code_block() {
    let r = convert("records.jsonl");
    assert_eq!(r.format, InputFormat::Jsonl);
    let (lang, text) = code_block(&r.document);
    assert_eq!(lang.as_deref(), Some("json"));
    assert_eq!(text.lines().count(), 3);
    assert!(text.contains("béta") && text.contains("日本語"));
    assert_eq!(
        InputFormat::from_extension("ndjson"),
        Some(InputFormat::Jsonl)
    );
    assert_eq!(
        InputFormat::from_mime("application/x-ndjson"),
        Some(InputFormat::Jsonl)
    );
}

/// An RSS feed is XML of no dialect docling reads: one `xml` code block
/// (with a warning on stderr), not an empty JATS document; the JATS, USPTO,
/// XBRL and DocLang sniffing of a bare `.xml` is unchanged.
#[test]
fn unknown_xml_dialect_is_an_xml_code_block() {
    let r = convert("feed.xml");
    assert_eq!(r.format, InputFormat::Xml);
    let (lang, text) = code_block(&r.document);
    assert_eq!(lang.as_deref(), Some("xml"));
    assert!(text.contains("<rss version=\"2.0\">") && text.contains("docling.rs 2.23.0"));
    assert!(r.document.export_to_markdown().starts_with("```\n<?xml"));
    assert!(r
        .document
        .export_to_markdown_with(true)
        .starts_with("```xml\n<?xml"));

    // JATS without a DOCTYPE, by its root element.
    let jats = b"<?xml version=\"1.0\"?><article><front><article-meta><title-group><article-title>A title</article-title></title-group></article-meta></front><body><sec><p>Body text.</p></sec></body></article>";
    let source = SourceDocument::from_bytes("paper.xml", InputFormat::Xml, jats.to_vec());
    let doc = DocumentConverter::new().convert(source).unwrap().document;
    assert!(
        !doc.nodes.iter().any(|n| matches!(n, Node::Code { .. })),
        "{:?}",
        doc.nodes
    );
    assert!(doc.export_to_markdown().contains("Body text."));
    assert_eq!(InputFormat::from_extension("xml"), Some(InputFormat::Xml));
    assert_eq!(
        InputFormat::from_extension("nxml"),
        Some(InputFormat::XmlJats)
    );
    assert_eq!(InputFormat::from_mime("text/xml"), Some(InputFormat::Xml));
}

/// A code extension becomes one code block in its language — a Python
/// script's `#` comments never turn into headings.
#[test]
fn code_files_become_code_blocks() {
    let r = convert("script.py");
    assert_eq!(r.format, InputFormat::Text);
    let (lang, text) = code_block(&r.document);
    assert_eq!(lang.as_deref(), Some("python"));
    assert!(text.starts_with("#!/usr/bin/env python3"));
    assert!(text.contains("    # Print the arguments, one per line."));
    assert!(
        !r.document.export_to_markdown().contains("\n# "),
        "a comment became a heading"
    );

    let (lang, text) = code_block(&convert("config.yaml").document);
    assert_eq!(lang.as_deref(), Some("yaml"));
    assert!(text.contains("port: 5001"));

    // No extension at all: the bytes are text, the name says the language.
    let (lang, text) = code_block(&convert("Makefile").document);
    assert_eq!(lang.as_deref(), Some("makefile"));
    assert!(text.contains("\t$(CC) -o main main.c"));
}

/// Anything else that is text — a log — reads as paragraphs, one per
/// blank-line-separated block with its lines kept.
#[test]
fn other_text_becomes_paragraphs() {
    let r = convert("app.log");
    assert_eq!(r.format, InputFormat::Text);
    let paras: Vec<&str> = r
        .document
        .nodes
        .iter()
        .map(|n| match n {
            Node::Paragraph { text } => text.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(paras.len(), 2);
    assert_eq!(paras[0].lines().count(), 2);
    assert!(paras[1].starts_with("2026-10-14 10:05:17 ERROR"));
    let md = r.document.export_to_markdown();
    assert!(md.contains("server started on 0.0.0.0:5001"));
}

/// An unknown extension whose bytes are text takes the text route; binary
/// junk still errors with the extension in the message.
#[test]
fn unknown_extensions_route_by_content() {
    let dir = std::env::temp_dir().join(format!("docling-671-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let notes = dir.join("notes.xyz");
    std::fs::write(&notes, "Just some notes.\n\nSecond paragraph, with café.\n").unwrap();
    let r = DocumentConverter::new()
        .convert(SourceDocument::from_file(&notes).unwrap())
        .unwrap();
    assert_eq!(r.format, InputFormat::Text);
    assert_eq!(r.document.nodes.len(), 2);
    assert!(r
        .document
        .export_to_markdown()
        .contains("Second paragraph, with café."));

    // Random bytes: not text, not a known signature.
    let junk = dir.join("random.bin");
    let bytes: Vec<u8> = (0..4096u32)
        .map(|i| (i.wrapping_mul(2654435761).rotate_left(13) >> 7) as u8)
        .collect();
    assert!(bytes.contains(&0), "the generator must produce NULs");
    std::fs::write(&junk, &bytes).unwrap();
    match SourceDocument::from_file(&junk) {
        Err(ConversionError::UnknownFormat { hint }) => assert!(hint.contains(".bin"), "{hint}"),
        other => panic!("{other:?}"),
    }
    std::fs::remove_dir_all(&dir).ok();

    // Nameless input: a media type of `text/*`, or text bytes, is text too.
    let src =
        SourceDocument::from_named_bytes("snippet", b"x = 1\n".to_vec(), Some("text/x-python"))
            .unwrap();
    assert_eq!(src.format, InputFormat::Text);
    let src = SourceDocument::from_named_bytes("stdin", b"plain words\n".to_vec(), None).unwrap();
    assert_eq!(src.format, InputFormat::Text);
    assert!(SourceDocument::from_named_bytes("blob", vec![0, 159, 146, 150, 0, 1], None).is_err());
    // … and `.py` by name keeps its language without a path.
    let src = SourceDocument::from_named_bytes("tool.py", b"print(1)\n".to_vec(), None).unwrap();
    assert_eq!(src.extension.as_deref(), Some("py"));
    let (lang, _) = code_block(&DocumentConverter::new().convert(src).unwrap().document);
    assert_eq!(lang.as_deref(), Some("python"));
}

/// `--list-input-formats` documents the fallback: the code and text
/// extensions, `jsonl`, `xml`; and the `text` identifier parses.
#[test]
fn supported_extensions_list_the_text_route() {
    let exts = InputFormat::supported_extensions();
    for e in [
        "py", "rs", "yaml", "sh", "log", "jsonl", "ndjson", "xml", "json",
    ] {
        assert!(exts.contains(&e), "{e} missing from the list");
    }
    assert_eq!(InputFormat::from_id("text"), Some(InputFormat::Text));
    assert_eq!(InputFormat::Text.as_str(), "text");
    assert!(InputFormat::Text.extensions().any(|e| e == "py"));
}
