//! Apple iWork input (#213, #318): the iWork backend pinned against committed
//! Markdown groundtruth over `tests/data/iwork/` (repo root). The backend is
//! pure Rust and deterministic — no models, no gating. The `.pages` fixtures
//! are a conformance corpus: their groundtruth is upstream docling's own
//! output (see the corpus README), the rest pins our Numbers/Keynote
//! extension. Regenerate after an intentional change with:
//!
//! ```bash
//! DOCLING_RS_REGEN=1 cargo test -p docling --test iwork
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/iwork")
}

#[test]
fn iwork_fixtures_match_groundtruth() {
    let regen = std::env::var_os("DOCLING_RS_REGEN").is_some();
    let sources = corpus().join("sources");
    let mut checked = 0;
    let mut entries: Vec<_> = fs::read_dir(&sources)
        .expect("iwork sources")
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();
    for path in entries {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let source = SourceDocument::from_file(&path).expect("iwork fixture");
        let result = DocumentConverter::new().convert(source);
        // docling's fixture: Pages encrypts members with a compression method
        // ZIP does not define instead of setting the encryption flag — the
        // error must still say "password-protected".
        if name.contains("password_protected") {
            let err = result.err().map(|e| e.to_string()).unwrap_or_default();
            assert!(
                err.contains("password-protected"),
                "{name}: expected a password-protected error, got: {err:?}"
            );
            checked += 1;
            continue;
        }
        let md = result
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .document
            .export_to_markdown();
        let gt_path = corpus().join("groundtruth").join(format!("{name}.md"));
        if regen {
            fs::write(&gt_path, &md).expect("write groundtruth");
        } else {
            let expected = fs::read_to_string(&gt_path)
                .unwrap_or_else(|_| panic!("{name}: missing groundtruth (DOCLING_RS_REGEN=1)"));
            assert_eq!(md, expected, "{name}: Markdown drifted from groundtruth");
        }
        checked += 1;
    }
    assert!(
        checked >= 9,
        "expected the full iwork corpus, saw {checked}"
    );
}

/// Both Tika fixtures are the same source document saved by different Pages
/// releases (docling's own cross-check): the IWA and the '09 XML readers must
/// agree on the shared body text and on the table grid.
#[test]
fn both_pages_generations_agree() {
    let convert = |name: &str| {
        let source = SourceDocument::from_file(corpus().join("sources").join(name)).unwrap();
        DocumentConverter::new().convert(source).unwrap().document
    };
    let modern = convert("pages_2013.pages");
    let legacy = convert("pages_iwork09.pages");
    for sentence in ["Sample pages document", "Some plain text to parse."] {
        assert!(
            modern.export_to_markdown().contains(sentence),
            "modern: {sentence}"
        );
        assert!(
            legacy.export_to_markdown().contains(sentence),
            "legacy: {sentence}"
        );
    }
    // Template placeholders (`sf:ghost-text`) never surface as content.
    assert!(!legacy
        .export_to_markdown()
        .contains("Lorem ipsum dolor sit amet"));
    let grid = |doc: &docling::DoclingDocument| {
        doc.nodes
            .iter()
            .find_map(|n| match n {
                docling::Node::Table(t) => Some(t.rows.clone()),
                _ => None,
            })
            .expect("a table")
    };
    let rows = grid(&modern);
    assert_eq!(rows, grid(&legacy));
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0], ["Column one", "Column two", "Column three"]);
    assert_eq!(rows[3][2], "Cell nine");
}

/// A pre-2013 Pages package (`index.xml`) with page furniture and a template
/// placeholder: only the body text survives, as in docling's IWA path.
#[test]
fn legacy_pages_furniture_and_ghost_text_stay_out() {
    let ns = "http://developer.apple.com/namespaces/sf";
    let xml = format!(
        r#"<?xml version="1.0"?>
<sl:document xmlns:sl="http://developer.apple.com/namespaces/sl" xmlns:sf="{ns}">
  <sf:stylesheet>
    <sf:paragraphstyle sf:name="Body" sf:ident="ps-body"/>
    <sf:paragraphstyle sf:name="Heading 1" sf:ident="ps-h1"/>
  </sf:stylesheet>
  <sf:text-storage>
    <sf:text-body>
      <sf:p sf:style="ps-h1">Real heading</sf:p>
      <sf:p sf:style="ps-body">Real body text.<sf:ghost-text>Lorem ipsum</sf:ghost-text> after.</sf:p>
    </sf:text-body>
    <sf:header><sf:text-body><sf:p>Running header</sf:p></sf:text-body></sf:header>
    <sf:footer><sf:text-body><sf:p>Page footer</sf:p></sf:text-body></sf:footer>
    <sf:footnotes><sf:text-storage><sf:text-body><sf:p>A footnote body</sf:p></sf:text-body></sf:text-storage></sf:footnotes>
  </sf:text-storage>
</sl:document>"#
    );
    let source = SourceDocument::from_bytes(
        "furniture.pages",
        docling::InputFormat::Pages,
        zip_with(&[("index.xml", xml.as_bytes())]),
    );
    let md = DocumentConverter::new()
        .convert(source)
        .unwrap()
        .document
        .export_to_markdown();
    assert_eq!(md, "## Real heading\n\nReal body text. after.\n");
}

/// A zip that is neither an IWA package nor an '09 document is reported as
/// such (docling's message), not as a generic parse failure.
#[test]
fn zip_without_pages_index_is_rejected() {
    let source = SourceDocument::from_bytes(
        "not_really.pages",
        docling::InputFormat::Pages,
        zip_with(&[("word/document.xml", b"<w:document/>")]),
    );
    let err = DocumentConverter::new().convert(source).unwrap_err();
    assert!(
        err.to_string().contains("not a Pages document"),
        "unexpected error: {err}"
    );
}

fn zip_with(members: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write;
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        for (name, bytes) in members {
            zip.start_file::<_, ()>(*name, Default::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }
    buf
}

/// Legacy (pre-2013) Numbers/Keynote packages carry `index.xml`, not IWA —
/// only Pages has an '09 reader (docling's), so the error must say so instead
/// of a generic parse failure.
#[test]
fn pre_iwa_package_reports_clearly() {
    // A minimal zip with only an index.xml member.
    let mut buf = Vec::new();
    {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        zip.start_file::<_, ()>("index.xml", Default::default())
            .unwrap();
        zip.write_all(b"<document/>").unwrap();
        zip.finish().unwrap();
    }
    let source = SourceDocument::from_bytes("old", docling::InputFormat::Keynote, buf);
    let err = DocumentConverter::new().convert(source).unwrap_err();
    assert!(
        err.to_string().contains("pre-2013"),
        "unexpected error: {err}"
    );
}
