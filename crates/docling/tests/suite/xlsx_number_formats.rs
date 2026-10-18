//! #634: a cell's text is what Excel displays — its value through its number
//! format, by docling PR #4628's rules. The expected strings are that PR's
//! own code (`_displayed_cell_text`, head `ab177e1`) run through openpyxl
//! 3.1.5 on these very workbooks, so the oracle is upstream, not us.
//!
//! Fixtures (own, `crates/docling/tests/data/{xlsx,xls}/sources`):
//! `xlsx_number_formats_min.xlsx`/`.xls` — the issue's repro, one value per
//! format family; `xlsx_number_formats.xlsx` — docling#4619's "Order" sheet
//! and the PR's review cases plus the formats the PR leaves raw;
//! `xlsx_number_formats_1904.xlsx` — the Mac 1904 date system.

use docling::{DoclingDocument, DocumentConverter, SourceDocument};
use docling_core::Node;
use std::path::PathBuf;

fn fixture(format: &str, name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(format)
        .join("sources")
        .join(name)
}

fn convert(format: &str, name: &str) -> DoclingDocument {
    let source = SourceDocument::from_file(fixture(format, name)).unwrap();
    DocumentConverter::new()
        .convert(source)
        .unwrap_or_else(|e| panic!("{name}: {e}"))
        .document
}

/// Every table's rows, in document order, looking through sheet groups and
/// the provenance wrappers each table sits in.
fn tables(nodes: &[Node]) -> Vec<Vec<Vec<String>>> {
    let mut out = Vec::new();
    for n in nodes {
        match n {
            Node::Group { children, .. } => out.extend(tables(children)),
            Node::Prov { inner, .. }
            | Node::Commented { inner, .. }
            | Node::Furniture { inner, .. } => out.extend(tables(std::slice::from_ref(inner))),
            Node::Table(t) => out.push(t.rows.clone()),
            _ => {}
        }
    }
    out
}

fn rows(rows: &[&[&str]]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|r| r.iter().map(|s| s.to_string()).collect())
        .collect()
}

/// The issue's repro: `0.0%` renders, `dd/mm/yyyy` renders, and — exactly
/// as the PR has it — `"EUR"`, `#,##0`, `00000` and `General` stay raw.
#[test]
fn reporter_workbook_matches_the_pr() {
    let doc = convert("xlsx", "xlsx_number_formats_min.xlsx");
    assert_eq!(
        tables(&doc.nodes),
        vec![rows(&[
            &["label", "value", "number_format"],
            &["percent 0.0%", "15.0%", "0.0%"],
            &["currency", "1234.5", "\"EUR\" #,##0.00"],
            &["date dd/mm/yyyy", "31/01/2024", "dd/mm/yyyy"],
            &["thousands", "1234567.891", "#,##0"],
            &["zero padded", "42", "00000"],
            &["General", "3.14159", "General"],
        ])]
    );
}

/// The same workbook saved as `.xls`: the BIFF `FORMAT`/`XF` records and the
/// cells' `ixfe` give the same codes — except the date, which this file's
/// writer stored as the built-in `mm-dd-yy` (14), hence the ISO rendering.
#[test]
fn reporter_xls_reads_biff_formats() {
    let doc = convert("xls", "xlsx_number_formats_min.xls");
    assert_eq!(
        tables(&doc.nodes),
        vec![rows(&[
            &["label", "value", "number_format"],
            &["percent 0.0%", "15.0%", "0.0%"],
            &["currency", "1234.5", "\"EUR\" #,##0.00"],
            &["date dd/mm/yyyy", "2024-01-31", "dd/mm/yyyy"],
            &["thousands", "1234567.891", "#,##0"],
            &["zero padded", "42", "00000"],
            &["General", "3.14159", "General"],
        ])]
    );
}

/// docling#4619's "Order" sheet (`$12.50`, `$125.00`, `$270.00`, `10%`,
/// `Feb-25`) and the PR's review table — ISO for the built-in 14/22,
/// `mm:ss` minutes, 12-hour clocks, quoted literals, negative and zero
/// sections, half-up rounding, `[$$-409]`, and the raw fallbacks.
#[test]
fn order_and_review_sheets_match_the_pr() {
    let doc = convert("xlsx", "xlsx_number_formats.xlsx");
    let found = tables(&doc.nodes);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(
        found[0],
        rows(&[
            &["Item", "Qty", "Price", "Line total"],
            &["Orange juice", "10", "$12.50", "$125.00"],
            &["Tea", "20", "$7.25", "$145.00"],
            &["", "", "Subtotal", "$270.00"],
            &["", "", "Discount", "10%"],
            &["", "", "Order month", "Feb-25"],
        ])
    );
    let review: Vec<(&str, &str)> = found[1]
        .iter()
        .skip(1)
        .map(|r| (r[0].as_str(), r[1].as_str()))
        .collect();
    assert_eq!(
        review,
        vec![
            ("Date", "2025-02-01"),
            ("Datetime 22", "2025-02-01 13:30:00"),
            ("Minutes", "07:45"),
            ("Afternoon", "1:30 PM"),
            ("Morning", "12:07 AM"),
            ("24h", "13:30"),
            ("Day month", "1-Feb-25"),
            ("Due", "Due Feb 1"),
            ("Due semicolon", "Due; Feb 1"),
            ("ISO", "2025-02-01"),
            ("Weekday", "Saturday, February 1, 2025"),
            ("Locale currency", "$12.50"),
            ("Negative parens", "($5.00)"),
            ("Negative red", "$-5.00"),
            ("Negative plain", "-$5.00"),
            ("Percent half up", "13%"),
            ("Percent neg", "-13%"),
            ("Percent 2dp", "1.25%"),
            ("Currency half up", "$13"),
            ("Currency neg half up", "-$13"),
            ("Currency group", "$1,234.57"),
            ("Zero dash", "-"),
            ("Zero empty", ""),
            ("Suffix", "12.50$"),
            ("Duration", "1 day, 2:07:00"),
            ("Locale date raw", "2025-02-01 00:00:00"),
            ("Conditional raw", "0.125"),
            ("Optional decimals raw", "12.5"),
            ("Digit literal raw", "12.5"),
            ("EUR raw", "1234.5"),
            ("Thousands raw", "1234567.891"),
            ("Padded raw", "42"),
            ("Large", "$123,456,789,012.35"),
            ("Tiny", "0.00%"),
            ("Text format", "3.5"),
            ("Bool", "True"),
            ("Scientific raw", "12345.678"),
            ("Seconds", "1:02:03 AM"),
            ("Timestamp 21", "5:06:07"),
            ("Date time text", "2024-01-02 00:00:00"),
            ("Date time text h", "2024-01-02 0:00:00"),
            ("Escaped digits", "1"),
        ]
    );
}

/// A workbook on the 1904 date system: the serials are read against that
/// epoch (`has_1904_epoch`), so the dates come out as Excel shows them.
#[test]
fn the_1904_epoch_is_honoured() {
    let doc = convert("xlsx", "xlsx_number_formats_1904.xlsx");
    assert_eq!(
        tables(&doc.nodes),
        vec![rows(&[
            &["Label", "Value"],
            &["Month", "Feb-25"],
            &["ISO", "2025-02-01 13:30"],
            &["Builtin 14", "1999-12-31"],
        ])]
    );
}
