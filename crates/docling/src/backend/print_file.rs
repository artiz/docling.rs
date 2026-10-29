//! Print files (`.prn`) — what DOS and Windows programs wrote when told to
//! "print to file", a docling.rs extension (#216); docling has no reader.
//!
//! Two kinds share the extension:
//!
//! * a **report** — Lotus 1-2-3's, dBase's or Quattro's "print to file", the
//!   Windows "Generic / Text Only" printer: plain fixed-width text;
//! * a **printer stream** for a PCL (HP LaserJet) or ESC/P (Epson
//!   dot-matrix) printer: the same text between escape sequences.
//!
//! Both read as their text. The escape sequences are taken out — PCL's
//! parameterized `ESC` `(`…`/` form (its `W` data blocks, raster and font
//! downloads, skipped by their declared length) and the PJL job header,
//! ESC/P's commands by their parameter counts (bit images by their
//! declared length) — and the printer's overstrike tricks undone: a
//! backspace overprints (`_⌫X` and `X⌫X` read `X`), a bare carriage return
//! starts the line over for bold or underline (its second pass is dropped).
//! A form feed starts a new page. PostScript is a program, not a text, and
//! is refused with a pointer to Ghostscript.
//!
//! The text decodes as UTF-8 when it is, else as code page 437, the DOS
//! default. Per page, blank-line-separated blocks become paragraphs —
//! except a block laid out in columns (most of its lines hold an inner run
//! of three or more spaces: a report's table), which stays a preformatted
//! block so the columns line up.

use docling_core::{DoclingDocument, Node};

use crate::backend::dos_codepage::{self, CP437};
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;

pub struct PrnBackend;

impl DeclarativeBackend for PrnBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let bytes = &source.bytes;
        let head = &bytes[..bytes.len().min(64)];
        if head.starts_with(b"%!") || contains(head, b"@PJL ENTER LANGUAGE=POSTSCRIPT") {
            return Err(ConversionError::Parse(
                "prn: a PostScript print file — a program, not text; convert it to PDF \
                 (Ghostscript: `ps2pdf file.prn`) and convert that"
                    .into(),
            ));
        }
        let text = strip_printer_codes(bytes);
        let text = match std::str::from_utf8(&text) {
            Ok(s) => s.to_string(),
            Err(_) => dos_codepage::decode(&text, &CP437),
        };
        let mut doc = DoclingDocument::new(&source.name);
        for (i, page) in text.split('\u{c}').enumerate() {
            if i > 0 && !doc.nodes.is_empty() && !page.trim().is_empty() {
                doc.push(Node::PageBreak);
            }
            page_nodes(page, &mut doc);
        }
        Ok(doc)
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// One page's blocks: paragraphs, or preformatted text for a block in
/// columns.
fn page_nodes(page: &str, doc: &mut DoclingDocument) {
    let mut block: Vec<&str> = Vec::new();
    let emit = |block: &mut Vec<&str>, doc: &mut DoclingDocument| {
        if block.is_empty() {
            return;
        }
        let columned = block
            .iter()
            .filter(|l| l.trim_start().contains("   "))
            .count()
            * 2
            >= block.len();
        if columned {
            // The block's common indent goes; the columns stay.
            let indent = block
                .iter()
                .map(|l| l.len() - l.trim_start().len())
                .min()
                .unwrap_or(0);
            let text = block
                .iter()
                .map(|l| l.get(indent..).unwrap_or("").trim_end())
                .collect::<Vec<_>>()
                .join("\n");
            doc.push(Node::Code {
                language: None,
                text,
                orig: None,
                pretty: None,
            });
        } else {
            let text = block
                .iter()
                .flat_map(|l| l.split_whitespace())
                .collect::<Vec<_>>()
                .join(" ");
            doc.push(Node::Paragraph {
                text: crate::backend::markdown::escape_text(&text),
            });
        }
        block.clear();
    };
    for line in page.lines() {
        if line.trim().is_empty() {
            emit(&mut block, doc);
        } else {
            block.push(line);
        }
    }
    emit(&mut block, doc);
}

/// The text of a printer stream: escape sequences and the PJL header out,
/// overstrikes resolved, line ends as `\n`, form feeds kept.
fn strip_printer_codes(bytes: &[u8]) -> Vec<u8> {
    // A bare CR is the line end only in a file that has no LF at all.
    let cr_is_eol = !bytes.contains(&b'\n');
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    // Where the current output line starts, and whether a bare CR has sent
    // the print head back over it (its second pass is dropped).
    let mut line_start = 0usize;
    let mut overprint = false;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        i += 1;
        match b {
            0x1b => i = skip_escape(bytes, i),
            b'\r' => {
                if cr_is_eol {
                    out.push(b'\n');
                    line_start = out.len();
                } else if bytes.get(i) != Some(&b'\n') {
                    overprint = true;
                }
            }
            b'\n' => {
                out.push(b'\n');
                line_start = out.len();
                overprint = false;
            }
            0x0c => {
                out.push(0x0c);
                line_start = out.len();
                overprint = false;
            }
            0x08 => {
                // Overstrike: the character after the backspace wins.
                if out.len() > line_start && !overprint {
                    out.pop();
                }
            }
            b'\t' if !overprint => out.push(b'\t'),
            b if b < 0x20 || b == 0x7f => {}
            b if !overprint => out.push(b),
            _ => {}
        }
        // A PJL job header line: `@PJL …` up to its line end.
        if bytes[i.saturating_sub(1)..].starts_with(b"@PJL") && out.ends_with(b"@") {
            out.pop();
            while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != 0x1b {
                i += 1;
            }
            if bytes.get(i) == Some(&b'\n') {
                i += 1;
            }
        }
    }
    out
}

/// Skip the escape sequence whose `ESC` ended at `i`; the index after it.
fn skip_escape(d: &[u8], mut i: usize) -> usize {
    let Some(&c) = d.get(i) else {
        return i;
    };
    i += 1;
    match c {
        // ESC/P bit images: `ESC *` m nL nH (PCL's `ESC *` takes a group
        // character from `` ` `` to `~` instead).
        b'*' if !matches!(d.get(i), Some(0x60..=0x7e)) => {
            let (Some(&m), Some(&lo), Some(&hi)) = (d.get(i), d.get(i + 1), d.get(i + 2)) else {
                return d.len();
            };
            let columns = usize::from(lo) | usize::from(hi) << 8;
            let per = match m {
                32..=40 => 3,
                71..=73 => 6,
                _ => 1,
            };
            (i + 3 + columns * per).min(d.len())
        }
        // ESC/P underline `ESC - n` (n = 0/1, as a byte or a digit).
        b'-' if matches!(d.get(i), Some(0 | 1 | b'0' | b'1')) => i + 1,
        // PCL: a parameterized sequence.
        0x21..=0x2f => skip_pcl(d, i),
        // ESC/P bit images with a 16-bit length: `ESC K/L/Y/Z` nL nH data.
        b'K' | b'L' | b'Y' | b'Z' => match (d.get(i), d.get(i + 1)) {
            (Some(&lo), Some(&hi)) => {
                (i + 2 + (usize::from(lo) | usize::from(hi) << 8)).min(d.len())
            }
            _ => d.len(),
        },
        // ESC/P lists ended by NUL (tab stops).
        b'D' | b'B' => match d[i..].iter().position(|&b| b == 0) {
            Some(n) => i + n + 1,
            None => d.len(),
        },
        // ESC/P `ESC C n` or `ESC C NUL n` (page length in lines / inches).
        b'C' => i + if d.get(i) == Some(&0) { 2 } else { 1 },
        // ESC/P commands with two parameter bytes.
        b'\\' => i + 2,
        // ESC/P commands with one parameter byte.
        b'W' | b'w' | b'x' | b'k' | b'l' | b'Q' | b'J' | b'3' | b'A' | b'N' | b'R' | b'S'
        | b'U' | b'p' | b'q' | b't' | b'j' | b'a' | b'r' | b'I' => i + 1,
        // Everything else is the command byte alone (PCL `ESC E`, `ESC 9`;
        // ESC/P `ESC @`, `ESC E`/`F`, `ESC 4`/`5`, …).
        _ => i,
    }
    .min(d.len())
}

/// A PCL parameterized sequence after `ESC` and its parameterized char:
/// an optional group char, then value fields each ended by a parameter
/// char — lower-case continues the sequence, upper-case ends it; `W`/`w`
/// announces as many data bytes as the value says.
fn skip_pcl(d: &[u8], mut i: usize) -> usize {
    if matches!(d.get(i), Some(0x60..=0x7e)) {
        i += 1;
    }
    loop {
        let start = i;
        while matches!(d.get(i), Some(b'0'..=b'9' | b'+' | b'-' | b'.')) {
            i += 1;
        }
        let value: usize = std::str::from_utf8(&d[start..i])
            .ok()
            .and_then(|s| s.trim_start_matches('+').parse::<f64>().ok())
            .map(|v| v.max(0.0) as usize)
            .unwrap_or(0);
        let Some(&p) = d.get(i) else {
            return d.len();
        };
        i += 1;
        if p == b'W' || p == b'w' {
            i = (i + value).min(d.len());
        }
        match p {
            0x40..=0x5e => return i,
            0x60..=0x7e => continue,
            // Not a parameter character: the sequence was malformed; stop
            // before the byte so it reads as text.
            _ => return i - 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::InputFormat;

    fn convert(bytes: &[u8]) -> Result<DoclingDocument, ConversionError> {
        PrnBackend.convert(&SourceDocument::from_bytes(
            "t.prn",
            InputFormat::Prn,
            bytes.to_vec(),
        ))
    }

    #[test]
    fn report_text_keeps_its_columns_and_prose_reflows() {
        let doc = convert(
            b"Quarterly report\r\nfor the board\r\n\r\nItem      Q1     Q2\r\nApples    10     12\r\nPears      3      4\r\n\x0cPage two text\r\n",
        )
        .unwrap();
        assert!(
            matches!(&doc.nodes[0], Node::Paragraph { text } if text == "Quarterly report for the board")
        );
        assert!(
            matches!(&doc.nodes[1], Node::Code { text, .. } if text == "Item      Q1     Q2\nApples    10     12\nPears      3      4"),
            "{:?}",
            doc.nodes[1]
        );
        assert!(matches!(doc.nodes[2], Node::PageBreak));
        assert!(matches!(&doc.nodes[3], Node::Paragraph { text } if text == "Page two text"));
    }

    #[test]
    fn printer_codes_go_and_overstrikes_resolve() {
        // PJL header, PCL reset / font select / raster data, ESC/P bold and
        // underline, backspace underline, CR bold pass, CP437 text.
        let mut stream =
            b"\x1b%-12345X@PJL JOB NAME=\"x\"\r\n@PJL ENTER LANGUAGE=PCL\r\n\x1bE\x1b(s0p12h10V"
                .to_vec();
        stream.extend_from_slice(b"\x1b*b3WABC");
        stream.extend_from_slice(b"Caf\x82 \x1bEbold\x1bF \x1b-\x01u\x1b-\x00 _\x08X\r\n");
        stream.extend_from_slice(b"Twice\rTwice\r\n\x1bK\x02\x00\xff\xffend\r\n");
        let doc = convert(&stream).unwrap();
        let text: Vec<&str> = doc
            .nodes
            .iter()
            .map(|n| match n {
                Node::Paragraph { text } => text.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(text, vec!["Café bold u X Twice end"]);
    }

    #[test]
    fn postscript_is_refused() {
        let err = convert(b"%!PS-Adobe-3.0\n").unwrap_err();
        assert!(err.to_string().contains("ps2pdf"), "{err}");
    }
}
