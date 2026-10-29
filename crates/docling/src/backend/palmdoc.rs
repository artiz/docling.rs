//! Palm e-texts (`.pdb`) — PalmDOC / AportisDoc, TealDoc and zTXT (Weasel
//! Reader), the text documents of Palm OS handhelds, a docling.rs
//! extension (#216); docling has no reader (LibreOffice reaches them
//! through libe-book).
//!
//! A Palm database: a 78-byte header (the database name, then the type and
//! creator codes at offset 60 that say what it holds) and a table of record
//! offsets. Record 0 describes the text, the records after it hold it:
//!
//! * **PalmDOC** (`TEXt`/`REAd`) and **TealDoc** (`TEXt`/`TlDc`): record 0
//!   gives the compression (1 none, 2 PalmDOC's LZ77) and the number of
//!   text records, each compressed on its own. TealDoc adds markup tags —
//!   a `<HEADER TEXT="…">` is a heading, a `<LINK TEXT="…">` its text, the
//!   rest (bookmarks, labels, rules, images, styles) carry no text.
//! * **zTXT** (`zTXT`/`GPlm`): record 0 gives the number of text records,
//!   which hold one zlib stream.
//!
//! The text is UTF-8 when it is, else Windows-1252 (Palm OS's Latin
//! charset). It reads as blank-line-separated paragraphs, its lines read
//! through — or, in a text with no blank lines, a paragraph per line. The
//! database name is the document's title. Other Palm e-book formats
//! (eReader / Peanut Press, Plucker, Mobipocket) are refused by name; a
//! `.pdb` that is no Palm database at all (a Protein Data Bank file is
//! text) reads as plain text.

use std::io::Read;

use docling_core::{DoclingDocument, Node};

use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;

pub struct PalmDocBackend;

/// The Palm database's records, its name, type and creator; `None` when
/// the bytes are not one.
struct Pdb<'a> {
    name: String,
    kind: [u8; 4],
    creator: [u8; 4],
    records: Vec<&'a [u8]>,
}

fn parse_pdb(d: &[u8]) -> Option<Pdb<'_>> {
    if d.len() < 78 {
        return None;
    }
    let n = usize::from(u16::from_be_bytes([d[76], d[77]]));
    if n == 0 || 78 + 8 * n > d.len() {
        return None;
    }
    let mut offs: Vec<usize> = (0..n)
        .map(|i| u32::from_be_bytes(d[78 + 8 * i..82 + 8 * i].try_into().unwrap()) as usize)
        .collect();
    // Offsets ascend within the file, past the header.
    if offs.windows(2).any(|w| w[0] > w[1]) || offs[0] < 78 + 8 * n || *offs.last()? > d.len() {
        return None;
    }
    offs.push(d.len());
    let records = offs.windows(2).map(|w| &d[w[0]..w[1]]).collect();
    let name_bytes = &d[..32];
    let name_end = name_bytes.iter().position(|&b| b == 0).unwrap_or(32);
    let (name, _, _) = encoding_rs::WINDOWS_1252.decode(&name_bytes[..name_end]);
    Some(Pdb {
        name: name.trim().to_string(),
        kind: d[60..64].try_into().unwrap(),
        creator: d[64..68].try_into().unwrap(),
        records,
    })
}

/// Whether `bytes` are a Palm e-text this backend reads.
pub(crate) fn looks_like_palmdoc(bytes: &[u8]) -> bool {
    bytes.len() >= 78
        && matches!(
            (&bytes[60..64], &bytes[64..68]),
            (b"TEXt", b"REAd") | (b"TEXt", b"TlDc") | (b"zTXT", b"GPlm")
        )
}

impl DeclarativeBackend for PalmDocBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let Some(pdb) = parse_pdb(&source.bytes) else {
            // Not a Palm database: a text `.pdb` (Protein Data Bank) reads
            // as text.
            if crate::sniff::looks_like_text(&source.bytes) {
                return crate::backend::TextBackend.convert(source);
            }
            return Err(ConversionError::Parse(
                "pdb: not a Palm database (nor text)".into(),
            ));
        };
        let raw = match (&pdb.kind, &pdb.creator) {
            (b"TEXt", b"REAd") | (b"TEXt", b"TlDc") => palmdoc_text(&pdb)?,
            (b"zTXT", b"GPlm") => ztxt_text(&pdb)?,
            (kind, creator) => {
                let what = match (kind, creator) {
                    (b"PNRd", b"PPrs") => "an eReader (Peanut Press) e-book",
                    (b"Data", b"Plkr") => "a Plucker e-book",
                    (b"BOOK", b"MOBI") => "a Mobipocket e-book",
                    _ => "a Palm database of another kind",
                };
                return Err(ConversionError::Parse(format!(
                    "pdb: {what} (type '{}', creator '{}') — only PalmDOC, TealDoc and \
                     zTXT texts are read",
                    String::from_utf8_lossy(kind),
                    String::from_utf8_lossy(creator)
                )));
            }
        };
        let text = match String::from_utf8(raw) {
            Ok(s) => s,
            Err(e) => encoding_rs::WINDOWS_1252
                .decode(e.as_bytes())
                .0
                .into_owned(),
        };
        let mut doc = DoclingDocument::new(&source.name);
        if !pdb.name.is_empty() {
            doc.metadata.title = Some(pdb.name.clone());
        }
        let teal = &pdb.creator == b"TlDc";
        paragraphs(&text, teal, &mut doc);
        Ok(doc)
    }
}

/// PalmDOC / TealDoc: record 0's compression and text-record count, the
/// records decompressed and joined.
fn palmdoc_text(pdb: &Pdb) -> Result<Vec<u8>, ConversionError> {
    let r0 = pdb.records[0];
    if r0.len() < 16 {
        return Err(ConversionError::Parse(
            "pdb: PalmDOC header too short".into(),
        ));
    }
    let compression = u16::from_be_bytes([r0[0], r0[1]]);
    let count = usize::from(u16::from_be_bytes([r0[8], r0[9]]));
    let mut out = Vec::new();
    for rec in pdb.records.iter().skip(1).take(count) {
        match compression {
            1 => out.extend_from_slice(rec),
            2 => decompress(rec, &mut out),
            c => {
                return Err(ConversionError::Parse(format!(
                    "pdb: unknown PalmDOC compression {c}"
                )))
            }
        }
    }
    Ok(out)
}

/// PalmDOC's LZ77: `0x01`–`0x08` copy that many literal bytes; `0x00`,
/// `0x09`–`0x7F` are themselves; `0xC0`–`0xFF` a space plus the byte with
/// its top bit cleared; `0x80`–`0xBF` with the next byte an 11-bit distance
/// and a 3-bit length (+3) back-reference.
fn decompress(rec: &[u8], out: &mut Vec<u8>) {
    let start = out.len();
    let mut i = 0usize;
    while i < rec.len() {
        let c = rec[i];
        i += 1;
        match c {
            0x01..=0x08 => {
                let n = usize::from(c).min(rec.len() - i);
                out.extend_from_slice(&rec[i..i + n]);
                i += n;
            }
            0x00..=0x7f => out.push(c),
            0xc0..=0xff => {
                out.push(b' ');
                out.push(c ^ 0x80);
            }
            0x80..=0xbf => {
                let Some(&next) = rec.get(i) else { break };
                i += 1;
                let x = (usize::from(c) << 8 | usize::from(next)) & 0x3fff;
                let (dist, len) = (x >> 3, (x & 7) + 3);
                // A distance reaching before this record is corrupt.
                if dist == 0 || dist > out.len() - start {
                    break;
                }
                for _ in 0..len {
                    out.push(out[out.len() - dist]);
                }
            }
        }
    }
}

/// zTXT: one zlib stream over the text records (a random-access file
/// flushes it per record, which a streaming inflate reads the same).
fn ztxt_text(pdb: &Pdb) -> Result<Vec<u8>, ConversionError> {
    let r0 = pdb.records[0];
    if r0.len() < 8 {
        return Err(ConversionError::Parse("pdb: zTXT header too short".into()));
    }
    let count = usize::from(u16::from_be_bytes([r0[2], r0[3]]));
    let size = u32::from_be_bytes(r0[4..8].try_into().unwrap()) as usize;
    let stream: Vec<u8> = pdb
        .records
        .iter()
        .skip(1)
        .take(count)
        .flat_map(|r| r.iter().copied())
        .collect();
    let mut out = Vec::with_capacity(size.min(64 << 20));
    // A truncated stream yields what it has.
    let _ = flate2::read::ZlibDecoder::new(stream.as_slice())
        .take(size.max(1) as u64 * 2)
        .read_to_end(&mut out);
    if out.is_empty() {
        return Err(ConversionError::Parse("pdb: zTXT stream unreadable".into()));
    }
    Ok(out)
}

/// The text's paragraphs: blank-line-separated blocks, their lines read
/// through — a paragraph per line when the text has no blank lines.
/// TealDoc's tags come out first (a `<HEADER>` a heading).
fn paragraphs(text: &str, teal: bool, doc: &mut DoclingDocument) {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let blank_separated = text.contains("\n\n");
    let blocks: Vec<&str> = if blank_separated {
        text.split("\n\n").collect()
    } else {
        text.split('\n').collect()
    };
    for block in blocks {
        let mut rest = block;
        let mut buf = String::new();
        if teal {
            // Pull the tags out, a header as its own heading.
            while let Some(open) = rest.find('<') {
                let Some(len) = rest[open..].find('>') else {
                    break;
                };
                let tag = &rest[open + 1..open + len];
                let (name, attrs) = tag.split_once(' ').unwrap_or((tag, ""));
                let upper = name.to_ascii_uppercase();
                if !matches!(
                    upper.as_str(),
                    "HEADER"
                        | "LINK"
                        | "BOOKMARK"
                        | "LABEL"
                        | "HRULE"
                        | "IMAGE"
                        | "STYLE"
                        | "TEALPAINT"
                ) {
                    buf.push_str(&rest[..open + 1]);
                    rest = &rest[open + 1..];
                    continue;
                }
                buf.push_str(&rest[..open]);
                rest = &rest[open + len + 1..];
                let text_attr = attr(attrs, "TEXT");
                match upper.as_str() {
                    "HEADER" => {
                        push_text(&std::mem::take(&mut buf), doc);
                        if let Some(t) = text_attr.filter(|t| !t.trim().is_empty()) {
                            doc.push(Node::Heading {
                                level: 2,
                                text: crate::backend::markdown::escape_text(t.trim()),
                            });
                        }
                    }
                    "LINK" => buf.push_str(text_attr.unwrap_or("")),
                    _ => {}
                }
            }
        }
        buf.push_str(rest);
        push_text(&buf, doc);
    }
}

/// `NAME="value"` (or `NAME=value`) in a TealDoc tag's attributes.
fn attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let upper = attrs.to_ascii_uppercase();
    let at = upper.find(&format!("{name}="))? + name.len() + 1;
    let v = &attrs[at..];
    if let Some(q) = v.strip_prefix('"') {
        Some(&q[..q.find('"').unwrap_or(q.len())])
    } else {
        Some(&v[..v.find(' ').unwrap_or(v.len())])
    }
}

fn push_text(text: &str, doc: &mut DoclingDocument) {
    let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if !words.is_empty() {
        doc.push(Node::Paragraph {
            text: crate::backend::markdown::escape_text(&words),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::InputFormat;

    /// A Palm database of `records` with the given type and creator.
    fn pdb(name: &str, kind: &[u8; 4], creator: &[u8; 4], records: &[Vec<u8>]) -> Vec<u8> {
        let mut d = vec![0u8; 78];
        d[..name.len()].copy_from_slice(name.as_bytes());
        d[60..64].copy_from_slice(kind);
        d[64..68].copy_from_slice(creator);
        d[76..78].copy_from_slice(&(records.len() as u16).to_be_bytes());
        let mut off = 78 + 8 * records.len() + 2;
        for r in records {
            d.extend_from_slice(&(off as u32).to_be_bytes());
            d.extend_from_slice(&[0, 0, 0, 0]);
            off += r.len();
        }
        d.extend_from_slice(&[0, 0]);
        for r in records {
            d.extend_from_slice(r);
        }
        d
    }

    fn header(compression: u16, len: u32, count: u16) -> Vec<u8> {
        let mut r = compression.to_be_bytes().to_vec();
        r.extend_from_slice(&[0, 0]);
        r.extend_from_slice(&len.to_be_bytes());
        r.extend_from_slice(&count.to_be_bytes());
        r.extend_from_slice(&4096u16.to_be_bytes());
        r.extend_from_slice(&[0, 0, 0, 0]);
        r
    }

    fn convert(bytes: Vec<u8>) -> Result<DoclingDocument, ConversionError> {
        PalmDocBackend.convert(&SourceDocument::from_bytes(
            "t.pdb",
            InputFormat::PalmDoc,
            bytes,
        ))
    }

    fn texts(doc: &DoclingDocument) -> Vec<String> {
        doc.nodes
            .iter()
            .map(|n| match n {
                Node::Paragraph { text } => text.clone(),
                Node::Heading { text, .. } => format!("# {text}"),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn palmdoc_lz77_and_paragraphs() {
        // "abcabcabc d" — literal run, a back-reference (dist 3, len 6) and
        // a space+char byte; then a second, uncompressed-style paragraph.
        let rec = vec![
            0x03,
            b'a',
            b'b',
            b'c',
            0x80,
            (3 << 3) | 3,
            0xe4,
            b'\n',
            b'\n',
            b'x',
            0xe9,
        ];
        let doc = convert(pdb("Book", b"TEXt", b"REAd", &[header(2, 14, 1), rec])).unwrap();
        assert_eq!(texts(&doc), vec!["abcabcabc d", "x i"]);
        assert_eq!(doc.metadata.title.as_deref(), Some("Book"));
    }

    #[test]
    fn tealdoc_headers_and_ztxt() {
        let text = b"<HEADER TEXT=\"Part One\" ALIGN=CENTER>\nFirst\nline.\n\nSee <LINK TEXT=\"here\" TAG=x>.<BOOKMARK NAME=\"b\">";
        let doc = convert(pdb(
            "T",
            b"TEXt",
            b"TlDc",
            &[header(1, text.len() as u32, 1), text.to_vec()],
        ))
        .unwrap();
        assert_eq!(texts(&doc), vec!["# Part One", "First line.", "See here."]);
        let body = b"One\nTwo";
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut z, body).unwrap();
        let stream = z.finish().unwrap();
        let mut r0 = 0x0128u16.to_be_bytes().to_vec();
        r0.extend_from_slice(&1u16.to_be_bytes());
        r0.extend_from_slice(&(body.len() as u32).to_be_bytes());
        r0.extend_from_slice(&[0; 12]);
        let doc = convert(pdb("Z", b"zTXT", b"GPlm", &[r0, stream])).unwrap();
        assert_eq!(texts(&doc), vec!["One", "Two"]);
    }

    #[test]
    fn other_palm_books_are_refused_and_text_pdb_reads_as_text() {
        let err = convert(pdb("P", b"Data", b"Plkr", &[vec![0; 8]])).unwrap_err();
        assert!(err.to_string().contains("Plucker"), "{err}");
        let doc = convert(b"HEADER    PROTEIN\nATOM      1  N   MET A   1\n".to_vec()).unwrap();
        assert!(!doc.nodes.is_empty());
    }
}
