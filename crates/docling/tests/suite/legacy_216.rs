//! #216, the long-tail legacy formats: what each one converts to, through
//! `DocumentConverter` and the extension table.

use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn markdown_of(name: &str, bytes: Vec<u8>) -> String {
    DocumentConverter::new()
        .convert(SourceDocument::from_named_bytes(name, bytes, None).expect("routed"))
        .expect("converts")
        .document
        .export_to_markdown()
}

/// A StarWriter 5 global document (`.sgl`) is the `StarWriterDocument`
/// stream a `.sdw` is, and reads the same.
#[test]
fn sgl_reads_as_starwriter() {
    let bytes = std::fs::read(
        repo_root().join("crates/docling/tests/data/staroffice5/sources/Writer_3.1.sdw"),
    )
    .unwrap();
    let sdw = markdown_of("w.sdw", bytes.clone());
    assert!(!sdw.trim().is_empty());
    assert_eq!(markdown_of("w.sgl", bytes), sdw);
}

/// Kingsoft Spreadsheets (`.et`, templates `.ett` / `.eth`) are BIFF8 in an
/// OLE container: the XLS reader's output, unchanged.
#[test]
fn kingsoft_et_reads_as_xls() {
    let bytes = std::fs::read(repo_root().join("tests/data/xls/sources/xlsx_01.xls")).unwrap();
    let xls = markdown_of("t.xls", bytes.clone());
    assert!(xls.contains('|'), "{xls}");
    for ext in ["et", "ett", "eth"] {
        assert_eq!(
            markdown_of(&format!("t.{ext}"), bytes.clone()),
            xls,
            "{ext}"
        );
    }
}
