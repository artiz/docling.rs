//! CGM (`.cgm`) — the Computer Graphics Metafile, ISO/IEC 8632, the
//! vector-drawing interchange format of technical illustration (aircraft
//! and automotive manuals, S1000D / ATA iSpec 2200 graphics, 1990s office
//! clip art), a docling.rs extension (#216); docling has no reader.
//!
//! A metafile is a drawing, so what a document converter can take from it
//! is its text: the labels, callouts and captions its `TEXT` /
//! `RESTRICTED TEXT` / `APPEND TEXT` elements place. Each text element is a
//! paragraph in drawing order, a text continued by `APPEND TEXT` (its
//! `not final` flag) joined into one; the metafile's name is the document
//! title, and a metafile of several pictures heads each one's text with
//! the picture's name. The geometry itself is not rendered.
//!
//! Two of the standard's three encodings are read:
//!
//! * **binary** (ISO 8632-3, by far the most common): big-endian 16-bit
//!   element headers — class (4 bits), id (7), parameter length (5; 31 =
//!   long form, partitioned) — with the parameters padded to even length.
//!   A text element's point is two VDCs, whose size follows the metafile
//!   descriptor (`VDC TYPE`, `INTEGER` / `REAL PRECISION`) and the picture's
//!   `VDC INTEGER` / `REAL PRECISION`; its string a length byte (255 = a
//!   16-bit length follows) and the bytes;
//! * **clear text** (ISO 8632-4): `BEGMF 'name';` … `TEXT 10,20 FINAL
//!   'label';` statements, ended by `;` or `/`, strings in `'` or `"`
//!   (doubled to escape), comments between `%`.
//!
//! The character encoding (8632-2) is refused by name. Strings are read as
//! UTF-8 when they are, else as Latin-1 (the default ISO 8859-1 G1 set).

use docling_core::{DoclingDocument, Node};

use crate::backend::markdown::escape_text;
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;

pub struct CgmBackend;

/// Whether `bytes` open like a binary CGM: a `BEGIN METAFILE` element
/// (class 0, id 1) whose string parameter's length byte fills it.
pub(crate) fn looks_like_binary_cgm(bytes: &[u8]) -> bool {
    let (Some(&b0), Some(&b1)) = (bytes.first(), bytes.get(1)) else {
        return false;
    };
    let header = u16::from_be_bytes([b0, b1]);
    let len = (header & 31) as usize;
    header & 0xffe0 == 0x0020
        && ((1..31).contains(&len) && bytes.get(2) == Some(&((len - 1) as u8)) || len == 31)
}

/// Whether `bytes` open like a clear-text CGM (`BEGMF`, after blanks and
/// comments).
pub(crate) fn looks_like_clear_text_cgm(bytes: &[u8]) -> bool {
    let mut rest = bytes;
    loop {
        rest = rest.trim_ascii_start();
        match rest.first() {
            Some(b'%') => match rest[1..].iter().position(|&b| b == b'%') {
                Some(end) => rest = &rest[end + 2..],
                None => return false,
            },
            _ => return rest.len() >= 5 && rest[..5].eq_ignore_ascii_case(b"BEGMF"),
        }
    }
}

impl DeclarativeBackend for CgmBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let bytes = &source.bytes[..];
        let metafile = if looks_like_clear_text_cgm(bytes) {
            clear_text(bytes)
        } else if bytes.len() >= 2 && u16::from_be_bytes([bytes[0], bytes[1]]) & 0xffe0 == 0x0020 {
            // Any BEGIN METAFILE header: a `.cgm` the sniff's stricter
            // check (the name's length byte) would not claim still reads.
            binary(bytes)
        } else if bytes.first().is_some_and(|&b| b == 0x30) {
            // BEGIN METAFILE in the character encoding is the byte 0x30
            // followed by 0x20.
            return Err(ConversionError::Parse(
                "cgm: a character-encoded metafile (ISO 8632-2) — only the binary and \
                 clear-text encodings are read"
                    .into(),
            ));
        } else {
            return Err(ConversionError::Parse(
                "cgm: not a binary or clear-text Computer Graphics Metafile".into(),
            ));
        };
        let mut doc = DoclingDocument::new(&source.name);
        let name = metafile.name.trim();
        if !name.is_empty() {
            doc.metadata.title = Some(name.to_string());
        }
        let several = metafile.pictures.len() > 1;
        for picture in metafile.pictures {
            if several && !picture.name.trim().is_empty() {
                doc.push(Node::Heading {
                    level: 2,
                    text: escape_text(picture.name.trim()),
                });
            }
            for text in picture.texts {
                let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if !text.is_empty() {
                    doc.push(Node::Paragraph {
                        text: escape_text(&text),
                    });
                }
            }
        }
        Ok(doc)
    }
}

#[derive(Debug, Default)]
struct Picture {
    name: String,
    texts: Vec<String>,
}

#[derive(Debug, Default)]
struct Metafile {
    name: String,
    pictures: Vec<Picture>,
}

impl Metafile {
    fn picture(&mut self) -> &mut Picture {
        if self.pictures.is_empty() {
            self.pictures.push(Picture::default());
        }
        self.pictures.last_mut().expect("just pushed")
    }

    /// A text element's string: a new text, or — after a `not final`
    /// one — its continuation.
    fn text(&mut self, s: String, continued: bool) {
        let texts = &mut self.picture().texts;
        match texts.last_mut() {
            Some(last) if continued => last.push_str(&s),
            _ => texts.push(s),
        }
    }
}

/// A CGM string's bytes as text: UTF-8 when valid, else Latin-1.
fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

// ------------------------------------------------------------------ binary

/// The binary encoding's number sizes (bytes) in force.
#[derive(Clone, Copy)]
struct Precision {
    integer: usize,
    vdc_real: bool,
    vdc_integer: usize,
    vdc_float: usize,
}

impl Default for Precision {
    fn default() -> Self {
        // ISO 8632-3's defaults: 16-bit integers, integer VDCs of 16 bits,
        // real VDCs 32-bit fixed point.
        Precision {
            integer: 2,
            vdc_real: false,
            vdc_integer: 2,
            vdc_float: 4,
        }
    }
}

impl Precision {
    fn vdc(&self) -> usize {
        if self.vdc_real {
            self.vdc_float
        } else {
            self.vdc_integer
        }
    }
}

/// A big-endian signed integer of `size` bytes at `o`.
fn int_at(p: &[u8], o: usize, size: usize) -> Option<i64> {
    let b = p.get(o..o + size)?;
    let mut v: i64 = if b.first()? & 0x80 != 0 { -1 } else { 0 };
    for &x in b {
        v = (v << 8) | x as i64;
    }
    Some(v)
}

/// A precision element's bit count as bytes (8, 16, 24 or 32 bits).
fn bits_to_bytes(bits: i64) -> Option<usize> {
    matches!(bits, 8 | 16 | 24 | 32).then_some(bits as usize / 8)
}

/// A `REAL PRECISION`'s size: the form (0 floating, 1 fixed) and the two
/// field widths — 9/23 or 12/52 floating, 16/16 or 32/32 fixed.
fn real_size(p: &[u8], integer: usize) -> Option<usize> {
    let form = int_at(p, 0, 2)?;
    let a = int_at(p, 2, integer)?;
    let b = int_at(p, 2 + integer, integer)?;
    match (form, a, b) {
        (0, 9, 23) | (1, 16, 16) => Some(4),
        (0, 12, 52) | (1, 32, 32) => Some(8),
        _ => None,
    }
}

/// A string parameter at `o`: its text and the offset past it.
fn string_at(p: &[u8], o: usize) -> Option<String> {
    let len = *p.get(o)? as usize;
    let (start, len) = if len == 255 {
        // Long form: a 16-bit length (its top bit flags a continuation,
        // whose partitions are read as one run).
        let l = int_at(p, o + 1, 2)? as usize & 0x7fff;
        (o + 3, l)
    } else {
        (o + 1, len)
    };
    let end = (start + len).min(p.len());
    Some(decode(p.get(start..end)?))
}

fn binary(d: &[u8]) -> Metafile {
    let mut mf = Metafile::default();
    let mut prec = Precision::default();
    let mut o = 0usize;
    let mut params = Vec::new();
    // Whether the last text element left its string open (`not final`).
    let mut open = false;
    while let Some(h) = d.get(o..o + 2) {
        let h = u16::from_be_bytes([h[0], h[1]]);
        o += 2;
        let (class, id) = (h >> 12, (h >> 5) & 0x7f);
        params.clear();
        let mut len = (h & 31) as usize;
        if len == 31 {
            // Long form: partitions of up to 32767 bytes, each after a
            // 16-bit header whose top bit says another follows.
            while let Some(ph) = d.get(o..o + 2) {
                let ph = u16::from_be_bytes([ph[0], ph[1]]);
                o += 2;
                let plen = (ph & 0x7fff) as usize;
                let end = (o + plen).min(d.len());
                params.extend_from_slice(&d[o..end]);
                o = end + (plen & 1);
                if ph & 0x8000 == 0 {
                    break;
                }
            }
        } else {
            let end = (o + len).min(d.len());
            params.extend_from_slice(&d[o..end]);
            len += len & 1;
            o += len;
        }
        let p = &params[..];
        match (class, id) {
            // END METAFILE.
            (0, 2) => break,
            (0, 1) => mf.name = string_at(p, 0).unwrap_or_default(),
            // BEGIN PICTURE: a new picture with the descriptor's VDC
            // precisions (the picture-scope elements reset).
            (0, 3) => {
                mf.pictures.push(Picture {
                    name: string_at(p, 0).unwrap_or_default(),
                    texts: Vec::new(),
                });
                open = false;
            }
            (1, 3) => prec.vdc_real = int_at(p, 0, 2) == Some(1),
            (1, 4) => {
                if let Some(n) = int_at(p, 0, prec.integer).and_then(bits_to_bytes) {
                    prec.integer = n;
                }
            }
            (3, 1) => {
                if let Some(n) = int_at(p, 0, prec.integer).and_then(bits_to_bytes) {
                    prec.vdc_integer = n;
                }
            }
            (3, 2) => {
                if let Some(n) = real_size(p, prec.integer) {
                    prec.vdc_float = n;
                }
            }
            // TEXT: point, final flag, string.
            (4, 4) | (4, 5) | (4, 6) => {
                let at = match id {
                    4 => 2 * prec.vdc(),
                    5 => 4 * prec.vdc(),
                    _ => 0,
                };
                let Some(fin) = int_at(p, at, 2) else {
                    continue;
                };
                if let Some(s) = string_at(p, at + 2) {
                    mf.text(s, id == 6 && open);
                }
                open = fin == 0;
            }
            _ => {}
        }
    }
    mf
}

// ------------------------------------------------------------- clear text

/// One clear-text statement: its keyword and its quoted strings, and
/// whether it says `NOTFINAL`.
struct Statement {
    keyword: String,
    strings: Vec<String>,
    not_final: bool,
}

/// The statements of a clear-text metafile: split at `;` / `/` outside
/// strings and comments.
fn statements(d: &[u8]) -> Vec<Statement> {
    let text = decode(d);
    let mut out = Vec::new();
    let mut words = String::new();
    let mut strings = Vec::new();
    let mut chars = text.chars().peekable();
    let mut finish = |words: &mut String, strings: &mut Vec<String>| {
        let mut it = words.split(|c: char| c.is_whitespace() || c == ',');
        let keyword = it
            .find(|w| !w.is_empty())
            .unwrap_or("")
            .to_ascii_uppercase();
        let not_final = words
            .split(|c: char| !c.is_ascii_alphabetic())
            .any(|w| w.eq_ignore_ascii_case("notfinal"));
        if !keyword.is_empty() {
            out.push(Statement {
                keyword,
                strings: std::mem::take(strings),
                not_final,
            });
        }
        words.clear();
        strings.clear();
    };
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' => {
                let mut s = String::new();
                while let Some(x) = chars.next() {
                    if x == c {
                        if chars.peek() == Some(&c) {
                            chars.next();
                            s.push(c);
                            continue;
                        }
                        break;
                    }
                    s.push(x);
                }
                strings.push(s);
                words.push(' ');
            }
            '%' => {
                for x in chars.by_ref() {
                    if x == '%' {
                        break;
                    }
                }
                words.push(' ');
            }
            ';' | '/' => finish(&mut words, &mut strings),
            _ => words.push(c),
        }
    }
    finish(&mut words, &mut strings);
    out
}

fn clear_text(d: &[u8]) -> Metafile {
    let mut mf = Metafile::default();
    let mut open = false;
    for st in statements(d) {
        let first = st.strings.first().cloned().unwrap_or_default();
        match st.keyword.as_str() {
            "ENDMF" => break,
            "BEGMF" => mf.name = first,
            "BEGPIC" => {
                mf.pictures.push(Picture {
                    name: first,
                    texts: Vec::new(),
                });
                open = false;
            }
            "TEXT" | "RESTRTEXT" | "APNDTEXT" => {
                if let Some(s) = st.strings.last() {
                    mf.text(s.clone(), st.keyword == "APNDTEXT" && open);
                }
                open = st.not_final;
            }
            _ => {}
        }
    }
    mf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(class: u16, id: u16, params: &[u8]) -> Vec<u8> {
        let mut out = ((class << 12) | (id << 5) | params.len() as u16)
            .to_be_bytes()
            .to_vec();
        out.extend_from_slice(params);
        if params.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    fn string(s: &str) -> Vec<u8> {
        let mut v = vec![s.len() as u8];
        v.extend_from_slice(s.as_bytes());
        v
    }

    fn texts(bytes: Vec<u8>) -> (Option<String>, Vec<String>) {
        let doc = CgmBackend
            .convert(&SourceDocument::from_bytes(
                "t.cgm",
                crate::InputFormat::Cgm,
                bytes,
            ))
            .unwrap();
        let texts = doc
            .nodes
            .iter()
            .map(|n| match n {
                Node::Paragraph { text } => text.clone(),
                Node::Heading { text, .. } => format!("# {text}"),
                other => panic!("{other:?}"),
            })
            .collect();
        (doc.metadata.title, texts)
    }

    #[test]
    fn binary_text_elements_with_32_bit_vdcs_and_appended_text() {
        let mut d = element(0, 1, &string("Drawing"));
        // VDC INTEGER PRECISION 32 bits: a point is 8 bytes.
        d.extend(element(0, 3, &string("p1")));
        d.extend(element(3, 1, &[0, 32]));
        let mut text = vec![0, 0, 0, 1, 0, 0, 0, 2, 0, 0]; // point, not final
        text.extend(string("Fuel "));
        d.extend(element(4, 4, &text));
        let mut append = vec![0, 1]; // final
        append.extend(string("pump"));
        d.extend(element(4, 6, &append));
        let mut restricted = vec![0; 16]; // dx, dy, point
        restricted.extend([0, 1]);
        restricted.extend(string("Valve"));
        d.extend(element(4, 5, &restricted));
        d.extend(element(0, 2, &[]));
        assert!(looks_like_binary_cgm(&d));
        let (title, texts) = texts(d);
        assert_eq!(title.as_deref(), Some("Drawing"));
        assert_eq!(texts, ["Fuel pump", "Valve"]);
    }

    #[test]
    fn clear_text_statements() {
        let src = b"% a comment; with a semicolon %\nBEGMF 'Parts';\nBEGPIC 'Front';\n\
            TEXT 10,20 NOTFINAL 'It''s ';\nAPNDTEXT FINAL \"a / label\";\n\
            BEGPIC 'Back'; TEXT (1,2) FINAL 'Rear'/ ENDPIC; ENDMF;";
        assert!(looks_like_clear_text_cgm(src));
        let (title, texts) = texts(src.to_vec());
        assert_eq!(title.as_deref(), Some("Parts"));
        assert_eq!(texts, ["# Front", "It's a / label", "# Back", "Rear"]);
    }

    #[test]
    fn truncated_metafiles_do_not_panic() {
        let mut d = element(0, 1, &string("Drawing"));
        let mut text = vec![0, 1, 0, 2, 0, 1];
        text.extend(string("Label"));
        d.extend(element(4, 4, &text));
        d.push(0xff);
        d.push(0xff);
        for end in 0..d.len() {
            let _ = binary(&d[..end]);
        }
        assert!(!looks_like_binary_cgm(b"PK\x03\x04"));
        assert!(!looks_like_clear_text_cgm(b"% only a comment"));
    }

    /// Every fixture, cut at every few bytes, converts or errors — never
    /// panics.
    #[test]
    fn truncated_files_do_not_panic() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/cgm/sources");
        for entry in std::fs::read_dir(dir).unwrap() {
            let bytes = std::fs::read(entry.unwrap().path()).unwrap();
            for end in (0..bytes.len()).step_by(97) {
                let _ = CgmBackend.convert(&SourceDocument::from_bytes(
                    "t.cgm",
                    crate::InputFormat::Cgm,
                    bytes[..end].to_vec(),
                ));
            }
        }
    }
}
