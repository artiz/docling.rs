//! CSV backend.
//!
//! Mirrors `docling.backend.csv_backend.CsvDocumentBackend`: sniff the delimiter
//! among `, ; \t | :` from the first line (falling back to comma), parse the
//! whole file with RFC-4180 quote handling, and emit one table whose width is
//! the widest row (ragged rows are padded). Row 0 is the header.

use csv::ReaderBuilder;
use docling_core::{DoclingDocument, Node, Table};

use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;

pub struct CsvBackend;

impl DeclarativeBackend for CsvBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let text = source.text()?;
        let text: &str = &text;
        let delimiter = detect_delimiter(text);
        // docling reads with `csv.reader(strict=True)` and, since 2.129
        // (docling#4260), turns its `csv.Error` into a load error rather
        // than letting it escape: a closing quote followed by anything but
        // the delimiter or a line end, or a quote still open at EOF. The
        // `csv` crate is lenient about both, so they are checked up front.
        check_strict_quoting(text, delimiter)
            .map_err(|e| ConversionError::Parse(format!("csv: malformed quoting — {e}")))?;

        let mut reader = ReaderBuilder::new()
            .delimiter(delimiter)
            .has_headers(false)
            .flexible(true)
            .from_reader(text.as_bytes());

        let mut rows: Vec<Vec<String>> = Vec::new();
        for record in reader.records() {
            let record = record.map_err(|e| ConversionError::with_source("csv", e))?;
            // Cell escaping (newlines, pipes) happens centrally in the serializer.
            rows.push(record.iter().map(str::to_string).collect());
        }

        let mut doc = DoclingDocument::new(&source.name);
        if !rows.is_empty() {
            let num_cols = rows.iter().map(Vec::len).max().unwrap_or(0);
            for row in &mut rows {
                row.resize(num_cols, String::new());
            }
            doc.push(Node::Table(Table {
                rows,
                location: None,
                structure: None,
                cell_blocks: None,
                cells: None,
                caption: None,
                caption_parent: Default::default(),
            }));
        }
        Ok(doc)
    }
}

/// Sniff the delimiter from the first line: the candidate (`, ; \t | :`) that
/// Python `csv`'s strict-mode errors: `'<delimiter>' expected after '"'`
/// when a quoted field's closing quote is followed by anything other than
/// the delimiter, a line end or EOF (`""` inside the field is an escaped
/// quote), and `unexpected end of data` when EOF arrives inside a quoted
/// field. A quote inside an *unquoted* field is ordinary text, as in Python.
fn check_strict_quoting(text: &str, delimiter: u8) -> Result<(), String> {
    let d = delimiter as char;
    let mut chars = text.chars().peekable();
    let mut field_start = true;
    let mut line = 1usize;
    while let Some(c) = chars.next() {
        if field_start && c == '"' {
            // Inside a quoted field.
            loop {
                match chars.next() {
                    None => {
                        return Err(format!(
                            "unexpected end of data in a quoted field (line {line})"
                        ))
                    }
                    Some('"') => match chars.peek() {
                        Some('"') => {
                            chars.next();
                        }
                        Some(&n) if n == d => {
                            chars.next();
                            field_start = true;
                            break;
                        }
                        Some('\n') | Some('\r') | None => {
                            field_start = true;
                            break;
                        }
                        Some(n) => {
                            return Err(format!(
                                "'{d}' expected after '\"' (line {line}), found {n:?}"
                            ))
                        }
                    },
                    Some('\n') => line += 1,
                    Some(_) => {}
                }
            }
            continue;
        }
        if c == '\n' {
            line += 1;
        }
        field_start = c == d || c == '\n' || c == '\r';
    }
    Ok(())
}

/// occurs most often wins. When the first line carries none — a quoted field
/// spanning several lines cuts it mid-quote (docling#3985, 2.123) — retry
/// over the first 4 KiB, which closes the quote; comma remains the default
/// when nothing matches at all. The first line stays authoritative otherwise:
/// widening the sample unconditionally would change the delimiter picked for
/// ragged files.
fn detect_delimiter(text: &str) -> u8 {
    const CANDIDATES: [u8; 5] = [b',', b';', b'\t', b'|', b':'];
    let count_best = |sample: &[u8]| {
        let mut best = b',';
        let mut best_count = 0usize;
        for &c in &CANDIDATES {
            let n = sample.iter().filter(|&&b| b == c).count();
            if n > best_count {
                best_count = n;
                best = c;
            }
        }
        (best, best_count)
    };
    let first = text.lines().next().unwrap_or("");
    match count_best(first.as_bytes()) {
        (best, n) if n > 0 => best,
        _ => {
            let bytes = text.as_bytes();
            count_best(&bytes[..bytes.len().min(4096)]).0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::InputFormat;

    fn convert(bytes: &[u8]) -> DoclingDocument {
        let src = SourceDocument::from_bytes("data", InputFormat::Csv, bytes.to_vec());
        CsvBackend.convert(&src).unwrap()
    }

    #[test]
    fn converts_csv_to_table() {
        let doc = convert(b"name,age\nAlice,30\nBob,25\n");
        assert_eq!(
            doc.export_to_markdown(),
            "| name   |   age |\n|--------|-------|\n| Alice  |    30 |\n| Bob    |    25 |\n"
        );
    }

    /// docling#4260 (2.129): Python's strict reader rejects a closing quote
    /// followed by text and a quote left open at EOF; both are load errors
    /// here too, while an escaped quote and a quote inside an unquoted field
    /// still read.
    #[test]
    fn malformed_quoting_is_a_load_error() {
        let bad = |s: &str| {
            let src = SourceDocument::from_bytes("t.csv", InputFormat::Csv, s.as_bytes().to_vec());
            CsvBackend.convert(&src).is_err()
        };
        assert!(bad("a,\"b\"c,d\n"));
        assert!(bad("a,\"open\n"));
        assert!(!bad("a,\"x \"\"y\"\" z\",c\n"));
        assert!(!bad("a,b\"c,d\n"));
        assert!(!bad("\"a\",\"b\"\n\"c\",\"d\""));
    }

    #[test]
    fn handles_quoted_comma() {
        // The quoted field keeps its embedded comma instead of splitting.
        let doc = convert(b"a,b\n1,\"Lozano, Dr\"\n");
        let Node::Table(table) = &doc.nodes[0] else {
            panic!("expected a table");
        };
        assert_eq!(
            table.rows[1],
            vec!["1".to_string(), "Lozano, Dr".to_string()]
        );
    }

    #[test]
    fn sniffs_semicolon_delimiter() {
        let doc = convert(b"a;b;c\n1;2;3\n");
        let Node::Table(table) = &doc.nodes[0] else {
            panic!("expected a table");
        };
        assert_eq!(table.rows[0], vec!["a", "b", "c"]);
    }
}
