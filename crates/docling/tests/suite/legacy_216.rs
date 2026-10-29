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

/// Hancom Hangul documents are told by content: an HWP 5 compound file by
/// its `FileHeader` + `DocInfo` streams, an HWPX package by its
/// `application/hwp+zip` mimetype — a nameless input converts as one.
#[test]
fn hwp_and_hwpx_are_sniffed_by_content() {
    let dir = repo_root().join("crates/docling/tests/data/hwp/sources");
    for name in ["hwplib_table.hwp", "hwpxlib_table.hwpx"] {
        let bytes = std::fs::read(dir.join(name)).unwrap();
        assert_eq!(
            docling::InputFormat::detect(&bytes),
            Some(docling::InputFormat::Hwp),
            "{name}"
        );
        let named = markdown_of(name, bytes.clone());
        assert!(named.contains('|'), "{named}");
        assert_eq!(markdown_of("upload.bin", bytes), named, "{name}");
    }
}

/// A Computer Graphics Metafile reads as the text its drawing places,
/// sniffed by its BEGIN METAFILE element when nameless.
#[test]
fn cgm_reads_its_text_and_is_sniffed() {
    let bytes = std::fs::read(
        repo_root().join("crates/docling/tests/data/cgm/sources/libreoffice_binary_corvette.cgm"),
    )
    .unwrap();
    assert_eq!(
        docling::InputFormat::detect(&bytes),
        Some(docling::InputFormat::Cgm)
    );
    let md = markdown_of("upload.bin", bytes);
    assert!(md.contains("GENIGRAPHICS"), "{md}");
}

/// Word for Macintosh and ClarisWorks are sniffed by their signatures —
/// classic Mac files often had no extension; MacWrite reads by its own.
#[test]
fn mac_word_processors_are_sniffed() {
    let dir = repo_root().join("crates/docling/tests/data/mac_wp/sources");
    for name in [
        "libreoffice_word_5.0.mcw",
        "libreoffice_clarisworks_1.0.cwk",
    ] {
        let bytes = std::fs::read(dir.join(name)).unwrap();
        assert_eq!(
            docling::InputFormat::detect(&bytes),
            Some(docling::InputFormat::MacWp),
            "{name}"
        );
        let md = markdown_of("upload", bytes);
        assert!(md.contains("EXERCICE"), "{name}: {md}");
    }
    let mw = std::fs::read(dir.join("libreoffice_macwrite_4.5.mw")).unwrap();
    assert!(markdown_of("t.mw", mw).contains("Exercice"));
}

/// Lotus Word Pro reads by content too: the `WordPro` / `LWP7` header.
#[test]
fn lotus_word_pro_is_sniffed() {
    let bytes = std::fs::read(
        repo_root().join("crates/docling/tests/data/lwp/sources/libreoffice_wordpro.lwp"),
    )
    .unwrap();
    assert_eq!(
        docling::InputFormat::detect(&bytes),
        Some(docling::InputFormat::Lwp)
    );
    let md = markdown_of("upload", bytes);
    assert!(md.contains("This line is bold"), "{md}");
}
