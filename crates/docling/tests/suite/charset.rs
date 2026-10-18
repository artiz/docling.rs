//! Statistical charset detection for text inputs (#675): MarkItDown's cp932
//! CSV, KOI8-R and GBK Markdown, the unchanged UTF-8 / windows-1252 rules,
//! and the `encoding` option's precedence.

use std::path::{Path, PathBuf};

use docling::{ConversionError, DocumentConverter, SourceDocument};

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(rel)
}

fn markdown(rel: &str) -> String {
    let source = SourceDocument::from_file(fixture(rel)).expect("fixture");
    let mut document = DocumentConverter::new()
        .convert(source)
        .expect("converts")
        .document;
    // MarkItDown's table spelling (the facade's default): unpadded cells.
    document.compact_tables = true;
    document.export_to_markdown()
}

/// The issue's acceptance: MarkItDown's `test_mskanji.csv` (cp932, no
/// hint) comes out with its header and rows intact.
#[test]
fn cp932_csv_reads_as_shift_jis() {
    let md = markdown("csv/sources/test_mskanji.csv");
    for row in [
        "| 名前 | 年齢 | 住所 |",
        "| 佐藤太郎 | 30 | 東京 |",
        "| 三木英子 | 25 | 大阪 |",
        "| 髙橋淳 | 35 | 名古屋 |",
    ] {
        assert!(md.contains(row), "{row}\n{md}");
    }
}

/// KOI8-R and GBK samples decode to their own letters: a heading, a list,
/// a paragraph — not mojibake and not an error.
#[test]
fn koi8r_and_gbk_markdown_decode() {
    let ru = markdown("md/sources/koi8r_report.md");
    assert!(ru.starts_with("# Отчёт о командировке\n"), "{ru}");
    assert!(ru.contains("- Заключить договор до конца месяца."), "{ru}");
    assert!(ru.contains("чеки приложены к отчёту."), "{ru}");

    let zh = markdown("md/sources/gbk_weekly.md");
    assert!(zh.starts_with("# 项目周报\n"), "{zh}");
    assert!(zh.contains("- 完成接口文档的修订"), "{zh}");
    assert!(zh.contains("详细数据见附件中的表格。"), "{zh}");
}

/// What does not change: UTF-8 is read as is, damaged UTF-8 is still an
/// error, a Western legacy file is still windows-1252.
#[test]
fn utf8_rules_are_unchanged() {
    let utf8 =
        SourceDocument::from_bytes("a.md", docling::InputFormat::Md, "# Café\n\nnaïve".into());
    assert_eq!(utf8.text().unwrap(), "# Café\n\nnaïve");

    // Mostly well-formed UTF-8 with one broken sequence: damaged, an error.
    let mut damaged = "# Café ünïcödé text\n".as_bytes().to_vec();
    damaged.push(0xE9);
    damaged.extend_from_slice(b" tail\n");
    let source = SourceDocument::from_bytes("d.md", docling::InputFormat::Md, damaged);
    let err = source.text().unwrap_err();
    assert!(matches!(err, ConversionError::Parse(_)), "{err}");
    assert!(err.to_string().contains("damaged UTF-8"), "{err}");

    // windows-1252: no UTF-8 among the high bytes, Western letters.
    let source = SourceDocument::from_bytes(
        "w.md",
        docling::InputFormat::Md,
        b"# R\xe9sum\xe9\n\nna\xefve caf\xe9 \x96 d\xe9j\xe0 vu\n".to_vec(),
    );
    assert_eq!(source.text().unwrap(), "# Résumé\n\nnaïve café – déjà vu\n");
}

/// The `encoding` option wins over the guess, on the converter and on the
/// source.
#[test]
fn the_encoding_option_wins_over_the_guess() {
    let bytes = std::fs::read(fixture("md/sources/koi8r_report.md")).unwrap();
    // Named as what it is: the same text as the guess.
    let named = SourceDocument::from_bytes("r.md", docling::InputFormat::Md, bytes.clone())
        .with_encoding(Some("koi8-r".into()));
    assert!(named.text().unwrap().starts_with("# Отчёт о командировке"));
    // Named as something else: that something else, no second-guessing.
    let forced = DocumentConverter::new()
        .encoding(Some("windows-1251".into()))
        .convert(SourceDocument::from_bytes(
            "r.md",
            docling::InputFormat::Md,
            bytes,
        ))
        .unwrap()
        .document
        .export_to_markdown();
    assert!(!forced.contains("Отчёт"), "{forced}");
    assert!(forced.starts_with("# "), "{forced}");
}
