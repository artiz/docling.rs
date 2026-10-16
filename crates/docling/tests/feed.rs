//! RSS / Atom / RDF feeds (#678): MarkItDown's `test_rss.xml` and real
//! feeds render as a titled document of items, by extension, media type
//! or a bare `.xml`'s content — where a feed used to come out as one XML
//! code block (or, as `.xml`, empty).

use std::path::{Path, PathBuf};

use docling::{DocumentConverter, InputFormat, SourceDocument};
use docling_core::Node;

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn convert(source: SourceDocument) -> docling::ConversionResult {
    DocumentConverter::new().convert(source).expect("converts")
}

fn headings(doc: &docling_core::DoclingDocument) -> Vec<(u8, String)> {
    doc.nodes
        .iter()
        .filter_map(|n| match n {
            Node::Heading { level, text } => Some((*level, text.clone())),
            _ => None,
        })
        .collect()
}

/// The ticket's acceptance: `test_rss.xml` (a bare `.xml`) gives the
/// channel title and a level-2 heading per item, no raw markup.
#[test]
fn markitdown_rss_matches_the_acceptance() {
    let r = convert(
        SourceDocument::from_file(repo("crates/docling-py/tests/data/markitdown/test_rss.xml"))
            .unwrap(),
    );
    assert_eq!(r.format, InputFormat::Xml, "a bare .xml, routed by content");
    let md = r.document.export_to_markdown();
    assert!(md.starts_with("# The Official Microsoft Blog\n"), "{md}");
    let h = headings(&r.document);
    assert_eq!(h[0], (1, "The Official Microsoft Blog".into()));
    assert!(
        h.iter()
            .any(|(l, t)| *l == 2 && t.starts_with("Ignite 2024")),
        "{h:?}"
    );
    assert_eq!(h.iter().filter(|(l, _)| *l == 2).count(), 10, "{h:?}");
    assert!(md.contains("\nPublished on: "), "{md}");
    assert!(!md.contains("<rss") && !md.starts_with("```"), "{md}");
    let json = r.document.export_to_json();
    assert!(json.contains("\"label\": \"title\""), "{json}");
}

/// Real feeds: Atom (the Rust and Go blogs), RSS 2.0 (W3C), RSS 1.0 RDF
/// (Debian) — each converts by its extension, with its title, items and
/// bodies; the Atom `summary` + `content` pair both read. An item's body
/// keeps its own headings (MarkItDown's reading too), so the entry titles
/// are checked by name.
#[test]
fn real_feeds_convert_by_extension() {
    let feed = |name: &str| {
        convert(
            SourceDocument::from_file(repo(&format!(
                "crates/docling/tests/data/feed/sources/{name}"
            )))
            .unwrap(),
        )
    };
    let rust = feed("rust_blog.atom");
    assert_eq!(rust.format, InputFormat::Feed);
    let md = rust.document.export_to_markdown();
    assert!(md.starts_with("# Rust Blog\n"), "{md}");
    assert!(md.contains("\nUpdated on: "), "{md}");
    let h2 = |doc: &docling_core::DoclingDocument| -> Vec<String> {
        headings(doc)
            .into_iter()
            .filter(|(l, _)| *l == 2)
            .map(|(_, t)| t)
            .collect()
    };
    let rust_h2 = h2(&rust.document);
    for title in [
        "Demoting i686 Windows targets to std-only",
        "Announcing Rust 1.99.0",
    ] {
        assert!(rust_h2.iter().any(|t| t == title), "{rust_h2:?}");
    }
    assert!(md.contains("Rust 1.100.0"), "{md}");

    let go = feed("go_blog.atom");
    let md = go.document.export_to_markdown();
    assert!(md.starts_with("# The Go Blog\n"), "{md}");
    let go_h2 = h2(&go.document);
    for title in ["Better HTTP/1 connection reuse", "Arch-specific SIMD in Go"] {
        assert!(go_h2.iter().any(|t| t == title), "{go_h2:?}");
    }
    assert!(md.contains("Go 1.27"), "{md}");

    let w3c = feed("w3c_news.rss");
    let md = w3c.document.export_to_markdown();
    assert!(md.starts_with("# W3C - News\n\nW3C News\n"), "{md}");
    assert!(h2(&w3c.document).len() >= 25, "{:?}", h2(&w3c.document));
    assert!(md.contains("Verifiable Credentials"), "{md}");
    // A relative link in a body resolves against the item's link.
    assert!(
        w3c.document
            .links
            .iter()
            .all(|(_, h)| h.starts_with("http") || h.starts_with('#') || h.starts_with("mailto:")),
        "{:?}",
        w3c.document.links
    );

    let debian = feed("debian_security.rdf");
    let md = debian.document.export_to_markdown();
    assert!(
        md.starts_with("# Debian Security\n\nDebian Security Advisories\n"),
        "{md}"
    );
    let debian_h2 = h2(&debian.document);
    assert_eq!(debian_h2.len(), 30, "{debian_h2:?}");
    assert_eq!(debian_h2[0], "DSA-6561-1 sogo - security update");
    assert!(md.contains("\nPublished on: "), "{md}");
    for r in [&rust, &go, &w3c, &debian] {
        assert!(!r.document.export_to_markdown().starts_with("```"));
    }
}

/// The routes in: the extensions, the media types, the format id, and a
/// bare `.xml` whose content is a feed; content detection still calls the
/// bytes XML (the `xml` route reads the dialect), and an `.rdf` that is
/// no feed stays an XML code block.
#[test]
fn feeds_route_by_extension_media_type_and_content() {
    for ext in ["rss", "atom", "rdf"] {
        assert_eq!(
            InputFormat::from_extension(ext),
            Some(InputFormat::Feed),
            "{ext}"
        );
    }
    assert_eq!(
        InputFormat::from_mime("application/rss+xml"),
        Some(InputFormat::Feed)
    );
    assert_eq!(
        InputFormat::from_mime("application/atom+xml"),
        Some(InputFormat::Feed)
    );
    assert_eq!(InputFormat::from_id("feed"), Some(InputFormat::Feed));
    assert_eq!(InputFormat::Feed.as_str(), "feed");
    assert!(InputFormat::supported_extensions().contains(&"rss"));

    let atom = b"<feed xmlns=\"http://www.w3.org/2005/Atom\"><title>T</title><entry><title>E</title><updated>2024-01-02T00:00:00Z</updated><content type=\"html\">&lt;p&gt;body&lt;/p&gt;</content></entry></feed>";
    let detected = SourceDocument::from_named_bytes("stdin", atom.to_vec(), None).unwrap();
    assert_eq!(detected.format, InputFormat::Xml);
    let md = convert(detected).document.export_to_markdown();
    assert_eq!(
        md,
        "# T\n\n## E\n\nUpdated on: 2024-01-02T00:00:00Z\n\nbody\n"
    );
    let by_mime =
        SourceDocument::from_named_bytes("stdin", atom.to_vec(), Some("application/atom+xml"))
            .unwrap();
    assert_eq!(by_mime.format, InputFormat::Feed);

    let not_a_feed = SourceDocument::from_bytes(
        "graph.rdf",
        InputFormat::Feed,
        b"<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description/></rdf:RDF>".to_vec(),
    );
    let md = convert(not_a_feed).document.export_to_markdown();
    assert!(
        md.starts_with("```") && md.contains("rdf:Description"),
        "{md}"
    );
}
