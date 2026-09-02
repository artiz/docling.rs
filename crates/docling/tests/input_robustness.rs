//! Input-robustness parity with docling 2.120.2–2.124 (#319): UTF-8 BOM
//! handling, CSV dialect detection, table-edge cases, crash-proofing.
//! Each test mirrors the upstream regression test of the fix it ports.

use docling::{DocumentConverter, InputFormat, SourceDocument};

fn convert(name: &str, format: InputFormat, bytes: &[u8]) -> docling_core::DoclingDocument {
    DocumentConverter::new()
        .convert(SourceDocument::from_bytes(name, format, bytes.to_vec()))
        .expect("conversion succeeds")
        .document
}

// --- UTF-8 BOM (docling#4098 CSV, docling#4109 md/asciidoc/webvtt/json) ----

#[test]
fn csv_bom_is_not_part_of_the_first_cell() {
    let doc = convert(
        "bom.csv",
        InputFormat::Csv,
        "\u{feff}Name,Age\nAlice,30\n".as_bytes(),
    );
    let md = doc.export_to_markdown().replace(' ', "");
    assert!(md.contains("|Name|Age|"), "BOM reached the header: {md}");
}

#[test]
fn markdown_bom_does_not_hide_the_first_heading() {
    let doc = convert(
        "bom.md",
        InputFormat::Md,
        "\u{feff}# Title\n\nBody.\n".as_bytes(),
    );
    let md = doc.export_to_markdown();
    assert!(md.starts_with("# Title"), "BOM hid the heading: {md:?}");
}

#[test]
fn asciidoc_bom_does_not_hide_the_document_title() {
    let doc = convert(
        "bom.adoc",
        InputFormat::Asciidoc,
        "\u{feff}= Document Title\n\nBody.\n".as_bytes(),
    );
    let md = doc.export_to_markdown();
    assert!(md.contains("Document Title"), "{md:?}");
    assert!(!md.contains('\u{feff}'), "BOM reached the output: {md:?}");
}

#[test]
fn webvtt_bom_does_not_invalidate_the_signature() {
    let doc = convert(
        "bom.vtt",
        InputFormat::Vtt,
        "\u{feff}WEBVTT\n\n00:00:01.000 --> 00:00:05.000\nHello there\n".as_bytes(),
    );
    assert!(doc.export_to_markdown().contains("Hello there"));
}

#[test]
fn docling_json_bom_does_not_fail_the_load() {
    let json = convert("x.md", InputFormat::Md, b"# Hi\n").export_to_json();
    let bytes = [b"\xef\xbb\xbf".to_vec(), json.into_bytes()].concat();
    let doc = convert("bom.json", InputFormat::JsonDocling, &bytes);
    assert!(doc.export_to_markdown().contains("# Hi"));
}

// --- CSV dialect: quoted field spanning lines (docling#3985) ---------------

#[test]
fn csv_quoted_newline_in_first_field_still_sniffs_the_delimiter() {
    let doc = convert(
        "quoted.csv",
        InputFormat::Csv,
        b"\"line one\nstill line one\";b;c\n1;2;3\n",
    );
    let md = doc.export_to_markdown().replace(' ', "");
    // Three columns, and the multi-line cell survives as one cell.
    assert!(md.contains("|1|2|3|"), "wrong delimiter: {md}");
}

// --- Markdown table edge pipes (docling#3817) ------------------------------

#[test]
fn markdown_table_row_without_trailing_pipe_keeps_the_last_cell() {
    let doc = convert(
        "t.md",
        InputFormat::Md,
        b"| Character | Name in German\n|---|---\n| Scrooge McDuck | Dagobert Duck\n",
    );
    let md = doc.export_to_markdown();
    assert!(
        md.contains("Name in German"),
        "last header cell dropped: {md}"
    );
    assert!(md.contains("Dagobert Duck"), "last data cell dropped: {md}");
}

#[test]
fn markdown_table_without_leading_pipes_is_a_table() {
    let doc = convert(
        "t.md",
        InputFormat::Md,
        b"Region | Q1\n--- | ---\nNorth | 10\n",
    );
    let md = doc.export_to_markdown().replace(' ', "");
    assert!(md.contains("|Region|Q1|"), "not parsed as a table: {md}");
    assert!(md.contains("|North|10|"), "{md}");
}

#[test]
fn markdown_table_without_leading_pipes_keeps_formatted_header_cells() {
    let doc = convert(
        "t.md",
        InputFormat::Md,
        b"**Region** | Q1\n--- | ---\nNorth | 10\n",
    );
    let md = doc.export_to_markdown().replace(' ', "");
    assert!(md.contains("Region"), "{md}");
    assert!(md.contains("|North|10|"), "{md}");
}

// --- AsciiDoc dedent to base level (docling#3826) --------------------------

#[test]
fn asciidoc_list_dedent_to_base_keeps_both_items() {
    let doc = convert("dedent.adoc", InputFormat::Asciidoc, b"  * a\n* b\n");
    let md = doc.export_to_markdown();
    assert!(md.contains("- a"), "{md}");
    assert!(md.contains("- b"), "{md}");
}

// --- HTML header-only rowspan table (docling#3827) -------------------------

#[test]
fn html_header_only_rowspan_table_keeps_the_cell() {
    let doc = convert(
        "t.html",
        InputFormat::Html,
        b"<table><tr><th rowspan='2'>h</th></tr></table>",
    );
    let md = doc.export_to_markdown();
    assert!(md.contains('h'), "cell lost: {md}");
}

// --- XML sniffing with an undecodable / cut head (docling#4038) ------------

#[test]
fn xml_sniff_survives_a_multibyte_char_straddling_the_window() {
    // Well-formed UTF-8 whose 4000-byte sniff window ends mid-codepoint —
    // slicing the head at a fixed byte offset must not panic.
    let head = "<?xml version=\"1.0\"?>\n<article><body><sec><p>";
    let padding = "a".repeat(4000 - head.len() - 1);
    let xml = format!("{head}{padding}é</p></sec></body></article>\n");
    assert!(!xml.is_char_boundary(4000), "fixture must straddle the cut");
    let doc = convert("split.xml", InputFormat::XmlJats, xml.as_bytes());
    assert!(doc.export_to_markdown().contains("aaa"), "content lost");
}

#[test]
fn xml_sniff_tolerates_a_non_utf8_document() {
    // A JATS article declared in ISO-8859-1 (0xF8 = ø). Format sniffing must
    // not abort; the strict backend may then fail cleanly, but never panic.
    let latin1: Vec<u8> = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>\n<!DOCTYPE article PUBLIC \"-//NLM//DTD JATS-journalpublishing1.dtd\" \"x.dtd\">\n<article><front><article-meta><contrib><name><surname>Bj\xf8rnstad</surname></name></contrib></article-meta></front></article>\n".to_vec();
    let _ = DocumentConverter::new().convert(SourceDocument::from_bytes(
        "latin1.xml",
        InputFormat::XmlJats,
        latin1,
    ));
}
