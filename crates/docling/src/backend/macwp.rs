//! Classic Macintosh word processors — MacWrite (`.mw`), Microsoft Word for
//! the Macintosh 1–5 (`.mcw`) and ClarisWorks / AppleWorks 1–6 (`.cwk`), a
//! docling.rs extension (#216); docling has none of them, LibreOffice
//! reads them through libmwaw, whose readers this module's layouts follow.
//! The three are told apart by content, whatever their extension (Mac files
//! often had none), and their text is Mac Roman:
//!
//! * **MacWrite** (1.0–2.2: version 3; 4.5–5.0: version 6) — a 40-byte
//!   header (version 6) with the paragraph counts of the main text, the
//!   header and the footer, the 120-byte print record, then one window
//!   record per text (footer, header, main) naming its *information* array:
//!   16-byte entries, one per paragraph — a ruler, a picture or a text, the
//!   text's block offset and length, its compression flag and default
//!   style. A text block is a character count, the characters — raw, or
//!   compressed as nibbles into `" etnroaisdlhcfp"` with `0xF` escaping a
//!   literal byte — and its style runs (bold, italic, underline,
//!   super/subscript). Version 3 lists 5-byte entries and the paragraph
//!   blocks after them, each with its type and size.
//! * **Word for Mac** — magic `FE32` (1.0), `FE34` (3.0), `FE37` `001C` /
//!   `0023` (4.0 / 5.0): the text runs from 0x80 (1.0) or 0x100 (3.0) to the
//!   header's end-of-text, or — 4.0 / 5.0 — through the piece table (`Clx`)
//!   of a fast-saved file, the main text, then the footnotes and the
//!   headers / footers by the lengths the header gives.
//! * **ClarisWorks** (`BOBO` at offset 4, version 1–6) — the document is a
//!   graph of `DSET` zones; a zone of file type 1 is a text: its header
//!   gives the character count and the number of text chunks, which follow
//!   its paragraph, font, token and size blocks. Zone 1 is the main text;
//!   a header, footer or footnote zone is furniture, a frame's or a table
//!   cell's text a paragraph after the main text.
//!
//! The paragraphs keep their text; MacWrite's character styles come through
//! (bold, italic, underline, sub/superscript), Word's and ClarisWorks' are
//! not read (their style sheets are left out). Pictures are not extracted.

use docling_core::{DoclingDocument, Node, Script};

use crate::backend::markdown::escape_text;
use crate::backend::styled::{Para, Style};
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;

pub struct MacWpBackend;

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

/// Mac Roman bytes as text.
fn mac_roman(bytes: &[u8]) -> String {
    encoding_rs::MACINTOSH
        .decode_without_bom_handling(bytes)
        .0
        .into_owned()
}

/// Whether `bytes` open like a Word for Macintosh 1–5 document.
pub(crate) fn looks_like_mac_word(bytes: &[u8]) -> bool {
    bytes.len() >= 0x80
        && matches!(
            (u16_at(bytes, 0), u16_at(bytes, 2)),
            (Some(0xfe32 | 0xfe34), Some(0)) | (Some(0xfe37), Some(0x1c | 0x23))
        )
}

/// Whether `bytes` open like a ClarisWorks / AppleWorks 1–6 document.
pub(crate) fn looks_like_clarisworks(bytes: &[u8]) -> bool {
    bytes.get(4..8) == Some(b"BOBO") && (1..=6).contains(&bytes[0])
}

impl DeclarativeBackend for MacWpBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let bytes = &source.bytes[..];
        let text = if looks_like_mac_word(bytes) {
            word(bytes)
        } else if looks_like_clarisworks(bytes) {
            clarisworks(bytes)?
        } else if matches!(u16_at(bytes, 0), Some(3 | 6)) {
            macwrite(bytes)?
        } else {
            return Err(ConversionError::Parse(
                "mac_wp: neither a MacWrite 1.0–5.0, a Word for Macintosh 1–5 nor a ClarisWorks 1–6 \
                 document (MacWrite Pro and MacWrite II are another format)"
                    .into(),
            ));
        };
        let mut doc = DoclingDocument::new(&source.name);
        text.emit(&mut doc);
        Ok(doc)
    }
}

/// A document's paragraphs, the body's and the furniture's.
#[derive(Default)]
struct Text {
    body: Vec<Block>,
    /// `(label, paragraph)`: `page_header`, `page_footer`, `footnote`.
    furniture: Vec<(&'static str, String)>,
}

enum Block {
    Para(Para),
    PageBreak,
}

impl Text {
    /// Plain text split into paragraphs at CR; a form feed is a page
    /// break, tabs and line breaks are spaces, other controls drop out.
    fn plain(&mut self, text: &str) {
        for line in text.split('\r') {
            let mut parts = line.split('\x0c').peekable();
            while let Some(part) = parts.next() {
                let mut para = Para::new();
                para.push(&part.replace(['\t', '\x0b'], " "), Style::default());
                self.body.push(Block::Para(para));
                if parts.peek().is_some() {
                    self.body.push(Block::PageBreak);
                }
            }
        }
    }

    fn furniture(&mut self, label: &'static str, text: &str) {
        for line in text.split(['\r', '\x0c']) {
            let line: String = line
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
            if !line.is_empty() {
                self.furniture.push((label, line));
            }
        }
    }

    fn emit(self, doc: &mut DoclingDocument) {
        for block in self.body {
            match block {
                Block::Para(p) => {
                    if let Some(node) = p.into_node() {
                        doc.push(node);
                    }
                }
                Block::PageBreak => doc.push(Node::PageBreak),
            }
        }
        for (label, text) in self.furniture {
            doc.push(Node::FurnitureText {
                label: label.into(),
                text: escape_text(&text),
            });
        }
    }
}

// --------------------------------------------------------------- MacWrite

/// The nibble alphabet of MacWrite's text compression.
const NIBBLES: &[u8; 15] = b" etnroaisdlhcfp";

/// One paragraph's place: the information entry's type, data block, and
/// (version 6) compression flag and default style.
struct Info {
    text: bool,
    begin: usize,
    len: usize,
    compressed: bool,
    style: Style,
}

fn style_bits(bold: bool, italic: bool, underline: bool, sup: bool, sub: bool) -> Style {
    Style {
        bold,
        italic,
        underline,
        strike: false,
        script: if sup {
            Script::Super
        } else if sub {
            Script::Sub
        } else {
            Script::Baseline
        },
    }
}

fn macwrite(d: &[u8]) -> Result<Text, ConversionError> {
    let version = u16_at(d, 0).unwrap_or(0);
    let bad = || ConversionError::Parse("mac_wp: a truncated MacWrite document".into());
    // The main text, the header, the footer.
    let mut zones: [Vec<Info>; 3] = Default::default();
    if version == 6 {
        // Window records after the 40-byte header and the print record,
        // in file order footer, header, main.
        for (record, zone) in [2usize, 1, 0].into_iter().enumerate() {
            let w = 40 + 120 + record * 46;
            let (Some(at), Some(len)) = (u32_at(d, w + 12), u16_at(d, w + 16)) else {
                return Err(bad());
            };
            let (at, len) = (at as usize, len as usize);
            let Some(entries) = d.get(at..at + len) else {
                continue;
            };
            for e in entries.chunks_exact(16) {
                let height = i16::from_be_bytes([e[0], e[1]]);
                let status = e[8];
                let format = u16::from_be_bytes([e[14], e[15]]);
                zones[zone].push(Info {
                    text: height > 0,
                    begin: (e[9] as usize) << 16 | u16::from_be_bytes([e[10], e[11]]) as usize,
                    len: u16::from_be_bytes([e[12], e[13]]) as usize,
                    compressed: status & 8 != 0,
                    style: style_bits(
                        format & 2 != 0,
                        format & 4 != 0,
                        format & 8 != 0,
                        format & 0x40 != 0,
                        format & 0x80 != 0,
                    ),
                });
            }
        }
    } else {
        // Version 3: the paragraph counts after the data offset, 5-byte
        // entries after the three 34-byte window records, then the
        // paragraph blocks from the data offset, zone by zone.
        let data = u16_at(d, 2).ok_or_else(bad)? as usize;
        let counts: Vec<usize> = (0..3)
            .map(|i| u16_at(d, 4 + 2 * i).unwrap_or(0) as usize)
            .collect();
        let mut o = data;
        for (zone, &count) in counts.iter().enumerate() {
            for _ in 0..count {
                let (Some(kind), Some(size)) = (u16_at(d, o), u16_at(d, o + 2)) else {
                    break;
                };
                let size = size as usize;
                if o + 4 + size > d.len() {
                    break;
                }
                zones[zone].push(Info {
                    text: kind & 7 == 1,
                    begin: o + 4,
                    len: size,
                    compressed: false,
                    style: Style::default(),
                });
                o += 4 + size;
            }
        }
    }
    let mut text = Text::default();
    for (zone, infos) in zones.iter().enumerate() {
        for info in infos.iter().filter(|i| i.text) {
            let Some(block) = d.get(info.begin..info.begin + info.len) else {
                continue;
            };
            let paras = macwrite_paragraph(block, info);
            match zone {
                0 => text.body.extend(paras.into_iter().map(Block::Para)),
                _ => {
                    let label = if zone == 1 {
                        "page_header"
                    } else {
                        "page_footer"
                    };
                    for p in paras {
                        text.furniture(label, &p.text());
                    }
                }
            }
        }
    }
    Ok(text)
}

/// A MacWrite text block: its characters and style runs, split at CR.
fn macwrite_paragraph(block: &[u8], info: &Info) -> Vec<Para> {
    let n = u16_at(block, 0).unwrap_or(0) as usize;
    let mut chars = Vec::with_capacity(n);
    let mut o = 2;
    if info.compressed {
        // Nibbles: an index into the alphabet, or 0xF and two nibbles of a
        // literal byte.
        let packed = block.get(2..).unwrap_or_default();
        let mut nibbles = packed.iter().flat_map(|b| [b >> 4, b & 0xf]);
        let mut used = 0usize;
        while chars.len() < n {
            let Some(x) = nibbles.next() else { break };
            used += 1;
            if x != 0xf {
                chars.push(NIBBLES[x as usize]);
                continue;
            }
            let (Some(hi), Some(lo)) = (nibbles.next(), nibbles.next()) else {
                break;
            };
            used += 2;
            chars.push(hi << 4 | lo);
        }
        o += used.div_ceil(2);
    } else {
        chars.extend_from_slice(block.get(2..2 + n).unwrap_or(&block[2.min(block.len())..]));
        o += chars.len();
    }
    o += o % 2;
    // Style runs: (position, size, flags, font).
    let mut runs: Vec<(usize, Style)> = Vec::new();
    if let Some(len) = u16_at(block, o) {
        if let Some(fmt) = block.get(o + 2..o + 2 + len as usize) {
            for f in fmt.chunks_exact(6) {
                let flags = f[3];
                runs.push((
                    u16::from_be_bytes([f[0], f[1]]) as usize,
                    style_bits(
                        flags & 1 != 0,
                        flags & 2 != 0,
                        flags & 4 != 0,
                        flags & 0x20 != 0,
                        flags & 0x40 != 0,
                    ),
                ));
            }
        }
    }
    let mut out = Vec::new();
    let mut para = Para::new();
    let mut style = info.style;
    let mut run = runs.iter().peekable();
    let decoded = mac_roman(&chars);
    for (i, c) in decoded.chars().enumerate() {
        while let Some(&&(pos, s)) = run.peek() {
            if pos > i {
                break;
            }
            style = s;
            run.next();
        }
        match c {
            '\r' => out.push(para.take()),
            '\t' => para.space(),
            _ => para.push_char(c, style),
        }
    }
    out.push(para);
    out
}

// ----------------------------------------------------------- Word for Mac

fn word(d: &[u8]) -> Text {
    let magic = u16_at(d, 0).unwrap_or(0);
    let mut text = Text::default();
    let (bot, eot) = match magic {
        0xfe32 => (0x80, u32_at(d, 14).unwrap_or(0) as usize),
        0xfe34 => (0x100, u32_at(d, 14).unwrap_or(0) as usize),
        _ => (
            u32_at(d, 20).unwrap_or(0x100) as usize,
            u32_at(d, 24).unwrap_or(0) as usize,
        ),
    };
    let eot = eot.min(d.len());
    let bot = bot.min(eot);
    // A fast-saved file's text is in pieces; the piece table is the Clx
    // entry of the zone list (entry 14 of 3.0's, 18 of 4.0 / 5.0's).
    let clx = match magic {
        0xfe32 => None,
        0xfe34 => Some(30 + 14 * 6),
        _ => Some(64 + 18 * 6),
    };
    let raw = clx
        .and_then(|e| pieces(d, e))
        .unwrap_or_else(|| d[bot..eot].to_vec());
    let all = mac_roman(&raw);
    // 4.0 / 5.0 give the main text's, the footnotes' and the headers'
    // lengths; the older versions keep everything in one run.
    let lengths: Vec<usize> = match magic {
        0xfe37 => (0..3)
            .map(|i| u32_at(d, 0x24 + 4 * i).unwrap_or(0) as usize)
            .collect(),
        _ => Vec::new(),
    };
    let chars: Vec<char> = all.chars().collect();
    match lengths.as_slice() {
        [main, notes, headers] if *main > 0 && *main <= chars.len() => {
            let main_end = *main;
            let notes_end = (main_end + notes).min(chars.len());
            let headers_end = (notes_end + headers).min(chars.len());
            text.plain(&chars[..main_end].iter().collect::<String>());
            text.furniture(
                "footnote",
                &chars[main_end..notes_end].iter().collect::<String>(),
            );
            text.furniture(
                "page_header",
                &chars[notes_end..headers_end].iter().collect::<String>(),
            );
        }
        _ => text.plain(&all),
    }
    text
}

/// The text of a fast-saved Word for Mac file, assembled from its piece
/// table: optional property blocks (type 1), then the pieces (type 2) —
/// N + 1 character positions and N 8-byte descriptors with the file
/// offset of each piece's bytes.
fn pieces(d: &[u8], entry: usize) -> Option<Vec<u8>> {
    let at = u32_at(d, entry)? as usize;
    let size = u16_at(d, entry + 4)? as usize;
    let zone = d.get(at..at + size)?;
    if size < 19 {
        return None;
    }
    let mut o = 0;
    while *zone.get(o)? == 1 {
        o += 3 + u16_at(zone, o + 1)? as usize;
    }
    if zone[o] != 2 {
        return None;
    }
    let sz = u16_at(zone, o + 1)? as usize;
    if sz % 12 != 4 {
        return None;
    }
    let n = sz / 12;
    let positions: Vec<usize> = (0..=n)
        .map(|i| u32_at(zone, o + 3 + 4 * i).map(|p| p as usize))
        .collect::<Option<_>>()?;
    let descriptors = o + 3 + 4 * (n + 1);
    let mut out = Vec::new();
    for i in 0..n {
        let len = positions[i + 1].checked_sub(positions[i])?;
        let ptr = u32_at(zone, descriptors + 8 * i + 2)? as usize;
        out.extend_from_slice(d.get(ptr..ptr + len)?);
    }
    Some(out)
}

// ------------------------------------------------------------ ClarisWorks

fn clarisworks(d: &[u8]) -> Result<Text, ConversionError> {
    let mut main: Option<String> = None;
    let mut others: Vec<(u8, String)> = Vec::new();
    let mut o = 0;
    while let Some(p) = d[o..].windows(4).position(|w| w == b"DSET").map(|p| p + o) {
        o = p + 4;
        // The zone header: size, data count, selection, text type, the
        // file type (1 = text), the zone id; then the text header —
        // character count and the number of text chunks.
        let (Some(size), Some(&file_type), Some(id)) =
            (u32_at(d, p + 4), d.get(p + 20), u16_at(d, p + 22))
        else {
            continue;
        };
        let end = p.saturating_add(8).saturating_add(size as usize);
        if file_type != 1 || end > d.len() || size < 30 {
            continue;
        }
        let kind = d[p + 18];
        let (Some(chars), Some(chunks)) = (u32_at(d, p + 28), u16_at(d, p + 32)) else {
            continue;
        };
        // The paragraph, font, token and size blocks, then the chunks.
        let mut b = end;
        let mut bytes = Vec::new();
        let mut complete = true;
        for z in 0..4 + chunks as usize {
            let Some(len) = u32_at(d, b) else {
                complete = false;
                break;
            };
            let Some(data) = d.get(b + 4..b + 4 + len as usize) else {
                complete = false;
                break;
            };
            if z >= 4 {
                bytes.extend_from_slice(data);
            }
            b += 4 + len as usize;
        }
        if !complete {
            continue;
        }
        bytes.truncate(chars as usize);
        let text = mac_roman(&bytes);
        if id == 1 && kind >> 4 == 0 && main.is_none() {
            main = Some(text);
        } else {
            others.push((kind >> 4, text));
        }
        o = b.max(o);
    }
    if main.is_none() && others.is_empty() {
        let kind = match d.get(match d[0] {
            1 => 242,
            2 | 3 => 248,
            4 => 256,
            5 => 268,
            _ => 278,
        }) {
            Some(2) => "a spreadsheet",
            Some(3) => "a database",
            Some(4) => "a painting",
            _ => "a document",
        };
        return Err(ConversionError::Parse(format!(
            "mac_wp: {kind} in ClarisWorks with no text zone — only its text is read"
        )));
    }
    let mut text = Text::default();
    if let Some(main) = main {
        text.plain(&main);
    }
    for (kind, t) in others {
        match kind {
            2 => text.furniture("page_header", &t),
            4 => text.furniture("page_footer", &t),
            6 => text.furniture("footnote", &t),
            _ => text.plain(&t),
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paragraphs(doc: &DoclingDocument) -> Vec<String> {
        doc.nodes
            .iter()
            .filter_map(|n| match n {
                Node::Paragraph { text } => Some(text.clone()),
                Node::InlineGroup { md_text, .. } => Some(md_text.clone()),
                Node::FurnitureText { label, text } => Some(format!("[{label}] {text}")),
                _ => None,
            })
            .collect()
    }

    fn convert(bytes: Vec<u8>) -> DoclingDocument {
        MacWpBackend
            .convert(&SourceDocument::from_bytes(
                "t.mw",
                crate::InputFormat::MacWp,
                bytes,
            ))
            .unwrap()
    }

    #[test]
    fn macwrite_nibble_compression_decodes() {
        // "rose" = r(4) o(5) s(8) e(1); "R" = F 5 2.
        let info = Info {
            text: true,
            begin: 0,
            len: 0,
            compressed: true,
            style: Style::default(),
        };
        let block = [0, 5, 0xf5, 0x24, 0x58, 0x10, 0, 0];
        let paras = macwrite_paragraph(&block, &info);
        assert_eq!(paras[0].text(), "Rrose");
    }

    #[test]
    fn macwrite_v3_paragraph_blocks() {
        // Version 3, data at 0x100, one main paragraph: a text block of
        // "Hi\r" with a bold run from 0.
        let mut d = vec![0u8; 0x100];
        d[1] = 3;
        d[2..4].copy_from_slice(&0x100u16.to_be_bytes());
        d[4..6].copy_from_slice(&1u16.to_be_bytes());
        let mut block = vec![0, 3, b'H', b'i', b'\r', 0];
        block.extend([0, 6, 0, 0, 12, 1, 0, 0]);
        d.extend((1u16).to_be_bytes());
        d.extend((block.len() as u16).to_be_bytes());
        d.extend(block);
        assert_eq!(paragraphs(&convert(d)), ["**Hi**"]);
    }

    #[test]
    fn word_for_mac_3_text_run() {
        let mut d = vec![0u8; 0x100];
        d[0..2].copy_from_slice(&0xfe34u16.to_be_bytes());
        let body = b"first\rsecond \x8et\x8e\x0cnext\r";
        d[14..18].copy_from_slice(&((0x100 + body.len()) as u32).to_be_bytes());
        d.extend_from_slice(body);
        assert!(looks_like_mac_word(&d));
        let doc = convert(d);
        assert_eq!(paragraphs(&doc), ["first", "second été", "next"]);
        assert!(doc.nodes.iter().any(|n| matches!(n, Node::PageBreak)));
    }

    #[test]
    fn clarisworks_text_zones() {
        let mut d = vec![1, 0, 0, 0];
        d.extend(b"BOBO");
        d.resize(300, 0);
        let mut dset = b"DSET".to_vec();
        let mut header = vec![0u8; 30];
        header[12] = 1; // file type (p + 20)
        header[15] = 1; // id 1 (p + 22..24)
        header[20..24].copy_from_slice(&7u32.to_be_bytes()); // chars (p + 28)
        header[24..26].copy_from_slice(&1u16.to_be_bytes()); // one chunk
        dset.extend((header.len() as u32).to_be_bytes());
        dset.extend(header);
        for _ in 0..4 {
            dset.extend(0u32.to_be_bytes());
        }
        dset.extend(8u32.to_be_bytes());
        dset.extend(b"one\rtwo\0");
        d.extend(dset);
        assert!(looks_like_clarisworks(&d));
        assert_eq!(paragraphs(&convert(d)), ["one", "two"]);
    }

    /// Every fixture, cut at every few bytes, converts or errors — never
    /// panics.
    #[test]
    fn truncated_files_do_not_panic() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/mac_wp/sources");
        for entry in std::fs::read_dir(dir).unwrap() {
            let bytes = std::fs::read(entry.unwrap().path()).unwrap();
            for end in (0..bytes.len()).step_by(13) {
                let _ = MacWpBackend.convert(&SourceDocument::from_bytes(
                    "t.mw",
                    crate::InputFormat::MacWp,
                    bytes[..end].to_vec(),
                ));
            }
        }
    }

    #[test]
    fn unknown_bytes_are_refused() {
        let err = MacWpBackend
            .convert(&SourceDocument::from_bytes(
                "t.mw",
                crate::InputFormat::MacWp,
                b"hello".to_vec(),
            ))
            .unwrap_err();
        assert!(err.to_string().contains("MacWrite"), "{err}");
    }
}
