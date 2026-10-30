//! What the PDF reader repaired to read a file (#934) — the typed value
//! `docling_pdf::pdf_repairs` returns and a conversion with
//! `pdf_report_repairs` carries on its result. It lives here, in the crate
//! every surface's result type already depends on, so the converter's
//! `ConversionResult` holds it whatever PDF features are compiled in.

use std::fmt;

use serde::Serialize;

/// One repair the reader makes (or has to give up on) to read the file.
///
/// Serialized (serve, Node, Python) as an object tagged by `kind`:
/// `{"kind": "stream_length", "object": [4, 0], "offset": 202, "declared": 5,
/// "recovered": 3}`, `{"kind": "xref_entries_padded"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PdfRepair {
    /// lopdf rejects the file, and it loads once the classic cross-reference
    /// table's 19-byte entries (a bare LF where the spec wants a two-byte
    /// EOL) are padded to 20 — the reader's fallback for parsers that refuse
    /// them (lopdf 0.45 itself reads such entries as they are; then no
    /// repair is taken and none is reported).
    XrefEntriesPadded,
    /// A stream whose declared `/Length` does not end at its `endstream`.
    StreamLength {
        /// The stream's object number and generation.
        object: (u32, u16),
        /// Byte offset of the object's `N G obj` header in the input.
        offset: usize,
        /// The declared length: the integer in the dictionary, or the one an
        /// indirect `/Length N G R` points at. `None` when it is missing,
        /// negative or not an integer, or the reference does not resolve.
        declared: Option<i64>,
        /// The length the reader recovers: the bytes up to the EOL before the
        /// one `endstream` followed by `endobj` (lopdf's rule, and what pdfium
        /// trusts). `None` when there is no such boundary or more than one —
        /// the stream is then dropped, not read.
        recovered: Option<usize>,
    },
}

impl fmt::Display for PdfRepair {
    /// One line for a log or a CLI: `stream 4 0 at byte 202: /Length 5,
    /// recovered 3`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PdfRepair::XrefEntriesPadded => {
                f.write_str("cross-reference entries padded to 20 bytes")
            }
            PdfRepair::StreamLength {
                object: (num, gen),
                offset,
                declared,
                recovered,
            } => {
                write!(f, "stream {num} {gen} at byte {offset}: /Length ")?;
                match declared {
                    Some(n) => write!(f, "{n}")?,
                    None => f.write_str("missing")?,
                }
                match recovered {
                    Some(n) => write!(f, ", recovered {n}"),
                    None => f.write_str(", no unambiguous end (stream dropped)"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_and_display_forms() {
        let r = PdfRepair::StreamLength {
            object: (4, 0),
            offset: 202,
            declared: Some(5),
            recovered: Some(3),
        };
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            r#"{"kind":"stream_length","object":[4,0],"offset":202,"declared":5,"recovered":3}"#
        );
        assert_eq!(
            r.to_string(),
            "stream 4 0 at byte 202: /Length 5, recovered 3"
        );
        assert_eq!(
            serde_json::to_string(&PdfRepair::XrefEntriesPadded).unwrap(),
            r#"{"kind":"xref_entries_padded"}"#
        );
    }
}
