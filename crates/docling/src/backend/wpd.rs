//! WordPerfect backend (`.wpd`/`.wp`/`.wp5`/`.wp6`/`.wpt`) — a docling.rs
//! extension (#216); docling has no WordPerfect reader (Python reaches it only
//! via LibreOffice's libwpd import filter).
//!
//! A WordPerfect file is a 16-byte prefix header (`ÿWPC`, a pointer to the
//! document area, product type, file type, major/minor version, encryption
//! flag) followed by prefix packets (fonts, styles, header/footer and note
//! text in 6.x) and then the *document area*: a byte stream mixing text with
//! function codes. Both generations parsed here share the shape and differ in
//! the code tables:
//!
//! - **WP 5.0/5.1** (major version 0): bytes 32–126 are ASCII, `0x0A` a hard
//!   return, `0x0B`/`0x0D` soft page/line breaks, `0x0C` a hard page break;
//!   `0x80`–`0xBF` single-byte functions (hard space `0xA0`, hard hyphens
//!   `0xA9`–`0xAB`, deletable soft returns `0x90`–`0x95`, dormant hard return
//!   `0x99`, hard return/soft page `0x8C`); `0xC0` an extended character
//!   (`C0 char charset C0` into the WP character sets); `0xC1`–`0xCF`
//!   fixed-length functions of a known size, among them attribute on/off
//!   (`C3 attr C3` / `C4 attr C4`); `0xD0`–`0xFF` variable-length groups
//!   (`code subgroup len16 … len16 subgroup code`, skipped by their length).
//! - **WP 6.x and later** (major version 2 — WP 6 through WordPerfect Office
//!   X-series share it): bytes 1–32 are the default extended-international
//!   characters, 33–126 ASCII; `0x80`–`0xCF` single-byte functions (soft
//!   space `0x80`, hard space `0x81`, hard hyphen `0x84`, dormant hard return
//!   `0x87`/`0x89`, hard end of line `0xCC` and its column/page variants,
//!   table cell/row ends `0xC6`–`0xCB`); `0xD0`–`0xEF` variable-length groups
//!   whose length counts from the code byte, the end-of-line group `0xD0`
//!   carrying soft/hard line ends and table cell/row/off marks in its
//!   subgroup; `0xF0` an extended character; `0xF1` the undo group (text
//!   between an undo start and end is *deleted* text and is dropped here);
//!   `0xF2`/`0xF3` attribute on/off; `0xF4`–`0xFE` fixed-length functions.
//!
//! Attributes 12/8/14/11/13/5/6 (bold, italic, underline, double underline,
//! strikeout, superscript, subscript) become inline runs; the rest (sizes,
//! small caps, redline, shadow, outline, blink) are ignored. Extended
//! characters map through the WP character sets 0–14 (Apache Tika's tables;
//! Multinational 1 #9, WordPerfect's typographic apostrophe, is mapped to
//! U+2019 rather than the combining comma the tables carry — in the DOS-era
//! corpus it is the possessive apostrophe of ordinary prose). Table cells and
//! rows collect into a [`Table`]; header/footer and footnote text (prefix
//! packets in 6.x, function payloads in 5.x) is not extracted. WP 4.2 and
//! earlier (no prefix header), Macintosh 3.x (file type 44) and encrypted
//! documents are refused with a targeted error.

use crate::backend::markdown::escape_text;
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;
use docling_core::{inline_paragraph_node, DoclingDocument, InlineRun, Node, Script, Table};

pub struct WpdBackend;

/// Every WordPerfect 5.0+ file starts with `0xFF "WPC"`.
const MAGIC: &[u8; 4] = b"\xffWPC";
/// Product type of WordPerfect proper (the same container carried other
/// Corel/WP products' files).
const PRODUCT_WORDPERFECT: u8 = 1;
/// File type of an ordinary document (as opposed to macros, dictionaries,
/// printer resources, … that share the prefix header).
const FILE_TYPE_DOCUMENT: u8 = 10;

/// The 16-byte prefix header.
struct Prefix {
    doc_area: usize,
    product: u8,
    file_type: u8,
    major: u8,
    minor: u8,
    encrypted: bool,
}

fn prefix(bytes: &[u8]) -> Result<Prefix, ConversionError> {
    if bytes.len() < 16 || &bytes[..4] != MAGIC {
        return Err(ConversionError::Parse(
            "wpd: not a WordPerfect document (no ÿWPC prefix; WordPerfect 4.2 and older \
             files have no header and are not supported)"
                .into(),
        ));
    }
    Ok(Prefix {
        doc_area: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize,
        product: bytes[8],
        file_type: bytes[9],
        major: bytes[10],
        minor: bytes[11],
        encrypted: u16::from_le_bytes([bytes[12], bytes[13]]) != 0,
    })
}

impl DeclarativeBackend for WpdBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let bytes = &source.bytes;
        let p = prefix(bytes)?;
        if p.product != PRODUCT_WORDPERFECT {
            return Err(ConversionError::Parse(format!(
                "wpd: not a WordPerfect document (product type {}; only WordPerfect's own \
                 files are supported)",
                p.product
            )));
        }
        if p.file_type != FILE_TYPE_DOCUMENT {
            return Err(ConversionError::Parse(format!(
                "wpd: unsupported WordPerfect file type {} (only documents are supported{})",
                p.file_type,
                if p.file_type == 44 {
                    "; this is a WordPerfect for Macintosh 3.x file"
                } else {
                    ""
                }
            )));
        }
        if p.encrypted {
            return Err(ConversionError::Parse(
                "wpd: password-protected WordPerfect document (encrypted documents are not \
                 supported)"
                    .into(),
            ));
        }
        let body = bytes.get(p.doc_area..).ok_or_else(|| {
            ConversionError::Parse(format!(
                "wpd: document area pointer {} beyond the file ({} bytes)",
                p.doc_area,
                bytes.len()
            ))
        })?;
        let mut b = Builder::new(&source.name);
        match p.major {
            0 => parse_wp5(body, &mut b),
            2 => parse_wp6(body, &mut b),
            _ => {
                return Err(ConversionError::Parse(format!(
                    "wpd: unsupported WordPerfect version {}.{} (5.x and 6.x+ documents are \
                     supported)",
                    p.major, p.minor
                )))
            }
        }
        Ok(b.finish())
    }
}

// ---------------------------------------------------------------------------
// Document builder: paragraphs of formatted runs, tables of cells.

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Fmt {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    script: Script,
}

struct Run {
    text: String,
    fmt: Fmt,
}

#[derive(Default)]
struct TableState {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
}

struct Builder {
    doc: DoclingDocument,
    runs: Vec<Run>,
    fmt: Fmt,
    table: Option<TableState>,
    /// Inside a WP6 undo region: the text is deleted, not document content.
    deleted: bool,
}

impl Builder {
    fn new(name: &str) -> Self {
        Self {
            doc: DoclingDocument::new(name),
            runs: Vec::new(),
            fmt: Fmt::default(),
            table: None,
            deleted: false,
        }
    }

    fn text(&mut self, s: &str) {
        if self.deleted || s.is_empty() {
            return;
        }
        match self.runs.last_mut() {
            Some(r) if r.fmt == self.fmt => r.text.push_str(s),
            _ => self.runs.push(Run {
                text: s.to_string(),
                fmt: self.fmt,
            }),
        }
    }

    fn ch(&mut self, c: char) {
        let mut buf = [0u8; 4];
        self.text(c.encode_utf8(&mut buf));
    }

    /// A soft line end (the line wrapped): one space, unless the line already
    /// ends in whitespace.
    fn soft_break(&mut self) {
        let ends_ws = self
            .runs
            .last()
            .and_then(|r| r.text.chars().last())
            .is_none_or(char::is_whitespace);
        if !ends_ws {
            self.text(" ");
        }
    }

    fn set_attr(&mut self, attr: u8, on: bool) {
        match attr {
            12 => self.fmt.bold = on,
            8 => self.fmt.italic = on,
            11 | 14 => self.fmt.underline = on,
            13 => self.fmt.strike = on,
            5 => self.fmt.script = if on { Script::Super } else { Script::Baseline },
            6 => self.fmt.script = if on { Script::Sub } else { Script::Baseline },
            _ => {}
        }
    }

    /// The pending runs as (Markdown, structured runs); `None` if only
    /// whitespace accumulated.
    fn take_runs(&mut self) -> Option<(String, Vec<InlineRun>)> {
        let runs = std::mem::take(&mut self.runs);
        let md = runs_markdown(&runs);
        if md.is_empty() {
            return None;
        }
        Some((md, runs_inline(&runs)))
    }

    /// A hard return. Inside a table cell it only separates lines of the
    /// cell's text.
    fn end_paragraph(&mut self) {
        if self.table.is_some() {
            self.soft_break();
            return;
        }
        if let Some((md, runs)) = self.take_runs() {
            self.doc.push(inline_paragraph_node(md, runs, false));
        }
    }

    fn end_cell(&mut self) {
        let text = runs_markdown(&std::mem::take(&mut self.runs));
        self.table
            .get_or_insert_with(TableState::default)
            .row
            .push(text);
    }

    fn end_row(&mut self) {
        let Some(t) = self.table.as_mut() else {
            // A row end with no table open is a line end.
            self.end_paragraph();
            return;
        };
        if !self.runs.is_empty() {
            let text = runs_markdown(&std::mem::take(&mut self.runs));
            t.row.push(text);
        }
        if !t.row.is_empty() {
            let row = std::mem::take(&mut t.row);
            t.rows.push(row);
        }
    }

    fn end_table(&mut self) {
        if self.table.is_none() {
            return;
        }
        self.end_row();
        let Some(t) = self.table.take() else {
            return;
        };
        if t.rows.is_empty() || t.rows.iter().all(|r| r.iter().all(|c| c.is_empty())) {
            return;
        }
        // Ragged rows pad to the widest, like the other declarative tables.
        let width = t.rows.iter().map(Vec::len).max().unwrap_or(0);
        let rows = t
            .rows
            .into_iter()
            .map(|mut r| {
                r.resize(width, String::new());
                r
            })
            .collect();
        self.doc.push(Node::Table(Table {
            rows,
            ..Default::default()
        }));
    }

    fn finish(mut self) -> DoclingDocument {
        self.end_table();
        self.end_paragraph();
        self.doc
    }
}

/// Markdown for a paragraph's runs: escaped text with bold/italic/strike
/// markers around each run's non-blank core (leading/trailing whitespace
/// stays outside the markers — `**x **` is not emphasis in CommonMark), tabs
/// as spaces, trimmed at both ends.
fn runs_markdown(runs: &[Run]) -> String {
    let mut out = String::new();
    for r in runs {
        let text = r.text.replace('\t', " ");
        let core = text.trim();
        if core.is_empty() {
            out.push_str(&text);
            continue;
        }
        let lead = &text[..text.len() - text.trim_start().len()];
        let trail = &text[text.trim_end().len()..];
        out.push_str(lead);
        let mut s = escape_text(core);
        if r.fmt.bold {
            s = format!("**{s}**");
        }
        if r.fmt.italic {
            s = format!("*{s}*");
        }
        if r.fmt.strike {
            s = format!("~~{s}~~");
        }
        out.push_str(&s);
        out.push_str(trail);
    }
    out.trim().to_string()
}

/// The structured [`InlineRun`]s (DocLang keeps underline and sub/superscript
/// that Markdown has no marker for).
fn runs_inline(runs: &[Run]) -> Vec<InlineRun> {
    runs.iter()
        .filter(|r| !r.text.trim().is_empty())
        .map(|r| InlineRun {
            text: r.text.replace('\t', " "),
            bold: r.fmt.bold,
            italic: r.fmt.italic,
            underline: r.fmt.underline,
            strike: r.fmt.strike,
            script: r.fmt.script,
            code: false,
            formula: false,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// WordPerfect 5.x document area.

/// Total sizes of the WP5 fixed-length functions `0xC0`–`0xCF`, code byte
/// included (spec order: extended character, center/align/tab/margin release,
/// indent, attribute on, attribute off, block protect, end of indent,
/// hyphenation display character, then reserved codes).
const WP5_FIXED: [usize; 16] = [4, 9, 11, 3, 3, 5, 6, 7, 4, 5, 6, 6, 8, 10, 10, 12];

fn parse_wp5(d: &[u8], b: &mut Builder) {
    let mut i = 0;
    while i < d.len() {
        let c = d[i];
        i += 1;
        match c {
            0x0A | 0x0C | 0x8C | 0x99 => b.end_paragraph(),
            0x0B | 0x0D | 0x90..=0x95 => b.soft_break(),
            0x09 => b.text("\t"),
            0x20..=0x7E => b.ch(c as char),
            0xA0 => b.ch('\u{a0}'),
            0xA9..=0xAB => b.ch('-'),
            0xC0 => {
                if let [val, set, ..] = d[i..] {
                    b.ch(wp_char(WpVersion::Wp5, set, val));
                }
                i += 3;
            }
            0xC3 | 0xC4 => {
                if let Some(&attr) = d.get(i) {
                    b.set_attr(attr, c == 0xC3);
                }
                i += 2;
            }
            0xC1..=0xCF => i += WP5_FIXED[(c - 0xC0) as usize] - 1,
            0xD0..=0xFF => {
                // subgroup, then the length of everything after these four
                // header bytes (payload + mirrored length/subgroup/code).
                let Some(len) = d.get(i + 1..i + 3) else {
                    break;
                };
                let len = u16::from_le_bytes([len[0], len[1]]) as usize;
                i += 3 + len;
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// WordPerfect 6.x+ document area.

/// Total sizes of the WP6 fixed-length functions `0xF0`–`0xFE`, code byte
/// included (extended character, undo, attribute on, attribute off, then
/// reserved codes).
const WP6_FIXED: [usize; 15] = [4, 5, 3, 3, 3, 3, 4, 4, 4, 5, 5, 6, 6, 8, 8];

fn parse_wp6(d: &[u8], b: &mut Builder) {
    let mut i = 0;
    while i < d.len() {
        let c = d[i];
        i += 1;
        match c {
            0x00 | 0x7F => {}
            0x01..=0x20 => b.ch(WP6_INTL[c as usize]),
            0x21..=0x7E => b.ch(c as char),
            0x80 => b.text(" "),
            0x81 => b.ch('\u{a0}'),
            0x84 | 0x85 => b.ch('-'),
            0x87 | 0x89 => b.end_paragraph(),
            // page-number display: skip to its closing pair
            0x8A => {
                while i < d.len() && d[i] != 0x8B {
                    i += 1;
                }
                i += 1;
            }
            0xC6 => b.end_cell(),
            0xC7..=0xCB => b.end_row(),
            0xB4..=0xC5 | 0xCC..=0xCF => b.end_paragraph(),
            0xD0..=0xEF => {
                // subgroup, then the length of the whole group counted from
                // the code byte.
                let Some(&[sub, l0, l1]) = d.get(i..i + 3) else {
                    break;
                };
                let len = u16::from_le_bytes([l0, l1]) as usize;
                if c == 0xD0 {
                    eol_group(sub, b);
                } else if c == 0xD5 {
                    b.soft_break();
                } else if c == 0xE0 {
                    b.text("\t");
                }
                i = (i - 1) + len.max(4);
            }
            0xF0 => {
                if let [val, set, ..] = d[i..] {
                    b.ch(wp_char(WpVersion::Wp6, set, val));
                }
                i += 3;
            }
            0xF1 => {
                match d.get(i) {
                    Some(0) => b.deleted = true,
                    Some(1) => b.deleted = false,
                    _ => {}
                }
                i += 4;
            }
            0xF2 | 0xF3 => {
                if let Some(&attr) = d.get(i) {
                    b.set_attr(attr, c == 0xF2);
                }
                i += 2;
            }
            0xF4..=0xFE => i += WP6_FIXED[(c - 0xF0) as usize] - 1,
            0xFF => {
                while i < d.len() && d[i] != 0xFF {
                    i += 1;
                }
                i += 1;
            }
            _ => {}
        }
    }
}

/// The WP6 end-of-line group's subgroups: soft line ends (the line wrapped)
/// become a space, hard ones a paragraph end, and the table marks build the
/// table — cell end, row end (with its end-of-column/page variants) and table
/// off.
fn eol_group(sub: u8, b: &mut Builder) {
    match sub {
        0..=3 | 20..=22 => b.soft_break(),
        10 => b.end_cell(),
        11..=14 => b.end_row(),
        15..=17 => b.end_table(),
        4..=9 | 18 | 19 | 23..=28 => b.end_paragraph(),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// WordPerfect character sets.

#[derive(Clone, Copy)]
enum WpVersion {
    Wp5,
    Wp6,
}

/// An extended character `(charset, value)` → Unicode; a space for an unknown
/// set or a value past the set's end (as Tika does).
fn wp_char(v: WpVersion, set: u8, val: u8) -> char {
    // WordPerfect's Multinational 1 #9 is its typographic apostrophe; the
    // table maps it to a combining mark, useless mid-word.
    if set == 1 && val == 9 {
        return '\u{2019}';
    }
    let table: Option<&[char]> = match (v, set) {
        (_, 0) => Some(WP6_CS0),
        (_, 1) => Some(WP6_CS1),
        (WpVersion::Wp5, 2) => Some(WP5_CS2),
        (WpVersion::Wp6, 2) => Some(WP6_CS2),
        (_, 3) => Some(WP6_CS3),
        (_, 4) => Some(WP6_CS4),
        (WpVersion::Wp5, 5) => Some(WP5_CS5),
        (WpVersion::Wp6, 5) => Some(WP6_CS5),
        (_, 6) => Some(WP6_CS6),
        (_, 7) => Some(WP6_CS7),
        (WpVersion::Wp5, 8) => Some(WP5_CS8),
        (WpVersion::Wp6, 8) => Some(WP6_CS8),
        (WpVersion::Wp5, 9) => Some(WP5_CS9),
        (WpVersion::Wp6, 9) => Some(WP6_CS9),
        (WpVersion::Wp5, 10) => Some(WP5_CS10),
        (WpVersion::Wp6, 10) => Some(WP6_CS10),
        (WpVersion::Wp5, 11) => Some(WP5_CS11),
        (WpVersion::Wp6, 11) => Some(WP6_CS11),
        (WpVersion::Wp5, 12) => Some(WP5_CS12),
        (WpVersion::Wp6, 12) => Some(WP6_CS12),
        (WpVersion::Wp6, 13) => Some(WP6_CS13),
        (WpVersion::Wp6, 14) => Some(WP6_CS14),
        _ => None,
    };
    match table.and_then(|t| t.get(val as usize)) {
        Some(&c) if c != '\0' => c,
        _ => ' ',
    }
}

// Tables after Apache Tika's WP5Charsets / WP6Charsets (Apache-2.0), which
// follow WordPerfect's published character-set appendix.

const WP6_INTL: &[char] = &[
    '\u{0000}', '\u{00e5}', '\u{00c5}', '\u{00e6}', '\u{00c6}', '\u{00e4}', '\u{00c4}', '\u{00e1}',
    '\u{00e0}', '\u{00e2}', '\u{00e3}', '\u{00c3}', '\u{00e7}', '\u{00c7}', '\u{00eb}', '\u{00e9}',
    '\u{00c9}', '\u{00e8}', '\u{00ea}', '\u{00ed}', '\u{00f1}', '\u{00d1}', '\u{00f8}', '\u{00d8}',
    '\u{00f5}', '\u{00d5}', '\u{00f6}', '\u{00d6}', '\u{00fc}', '\u{00dc}', '\u{00fa}', '\u{00f9}',
    '\u{00df}',
];

const WP6_CS0: &[char] = &[
    ' ', '!', '"', '#', '$', '%', '&', '\'', '(', ')', '*', '+', ',', '-', '.', '/', '0', '1', '2',
    '3', '4', '5', '6', '7', '8', '9', ':', ';', '<', '=', '>', '?', '@', 'A', 'B', 'C', 'D', 'E',
    'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X',
    'Y', 'Z', '[', '\\', ']', '^', '_', '`', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k',
    'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '{', '|', '}', '~',
    '\u{00a0}',
];

const WP6_CS1: &[char] = &[
    '\u{0300}', '\u{00b7}', '\u{0303}', '\u{0302}', '\u{0335}', '\u{0338}', '\u{0301}', '\u{0308}',
    '\u{0304}', '\u{0313}', '\u{0315}', '\u{02bc}', '\u{0326}', '\u{0315}', '\u{00b0}', '\u{0307}',
    '\u{030b}', '\u{0327}', '\u{0328}', '\u{030c}', '\u{0337}', '\u{0305}', '\u{0306}', '\u{00df}',
    '\u{0138}', 'j', '\u{00c1}', '\u{00e1}', '\u{00c2}', '\u{00e2}', '\u{00c4}', '\u{00e4}',
    '\u{00c0}', '\u{00e0}', '\u{00c5}', '\u{00e5}', '\u{00c6}', '\u{00e6}', '\u{00c7}', '\u{00e7}',
    '\u{00c9}', '\u{00e9}', '\u{00ca}', '\u{00ea}', '\u{00cb}', '\u{00eb}', '\u{00c8}', '\u{00e8}',
    '\u{00cd}', '\u{00ed}', '\u{00ce}', '\u{00ee}', '\u{00cf}', '\u{00ef}', '\u{00cc}', '\u{00ec}',
    '\u{00d1}', '\u{00f1}', '\u{00d3}', '\u{00f3}', '\u{00d4}', '\u{00f4}', '\u{00d6}', '\u{00f6}',
    '\u{00d2}', '\u{00f2}', '\u{00da}', '\u{00fa}', '\u{00db}', '\u{00fb}', '\u{00dc}', '\u{00fc}',
    '\u{00d9}', '\u{00f9}', '\u{0178}', '\u{00ff}', '\u{00c3}', '\u{00e3}', '\u{0110}', '\u{0111}',
    '\u{00d8}', '\u{00f8}', '\u{00d5}', '\u{00f5}', '\u{00dd}', '\u{00fd}', '\u{00d0}', '\u{00f0}',
    '\u{00de}', '\u{00fe}', '\u{0102}', '\u{0103}', '\u{0100}', '\u{0101}', '\u{0104}', '\u{0105}',
    '\u{0106}', '\u{0107}', '\u{010c}', '\u{010d}', '\u{0108}', '\u{0109}', '\u{010a}', '\u{010b}',
    '\u{010e}', '\u{010f}', '\u{011a}', '\u{011b}', '\u{0116}', '\u{0117}', '\u{0112}', '\u{0113}',
    '\u{0118}', '\u{0119}', '\u{01f4}', '\u{01f5}', '\u{011e}', '\u{011f}', '\u{01e6}', '\u{01e7}',
    '\u{0122}', '\u{0123}', '\u{011c}', '\u{011d}', '\u{0120}', '\u{0121}', '\u{0124}', '\u{0125}',
    '\u{0126}', '\u{0127}', '\u{0130}', 'i', '\u{012a}', '\u{012b}', '\u{012e}', '\u{012f}',
    '\u{0128}', '\u{0129}', '\u{0132}', '\u{0133}', '\u{0134}', '\u{0135}', '\u{0136}', '\u{0137}',
    '\u{0139}', '\u{013a}', '\u{013d}', '\u{013e}', '\u{013b}', '\u{013c}', '\u{013f}', '\u{0140}',
    '\u{0141}', '\u{0142}', '\u{0143}', '\u{0144}', '\u{0000}', '\u{0149}', '\u{0147}', '\u{0148}',
    '\u{0145}', '\u{0146}', '\u{0150}', '\u{0151}', '\u{014c}', '\u{014d}', '\u{0152}', '\u{0153}',
    '\u{0154}', '\u{0155}', '\u{0158}', '\u{0159}', '\u{0156}', '\u{0157}', '\u{015a}', '\u{015b}',
    '\u{0160}', '\u{0161}', '\u{015e}', '\u{015f}', '\u{015c}', '\u{015d}', '\u{0164}', '\u{0165}',
    '\u{0162}', '\u{0163}', '\u{0166}', '\u{0167}', '\u{016c}', '\u{016d}', '\u{0170}', '\u{0171}',
    '\u{016a}', '\u{016b}', '\u{0172}', '\u{0173}', '\u{016e}', '\u{016f}', '\u{0168}', '\u{0169}',
    '\u{0174}', '\u{0175}', '\u{0176}', '\u{0177}', '\u{0179}', '\u{017a}', '\u{017d}', '\u{017e}',
    '\u{017b}', '\u{017c}', '\u{014a}', '\u{014b}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}',
    '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}',
    '\u{0000}', '\u{0000}', '\u{1ef2}', '\u{1ef3}', '\u{010e}', '\u{010f}', '\u{01a0}', '\u{01a1}',
    '\u{01af}', '\u{01b0}', '\u{0114}', '\u{0115}', '\u{012c}', '\u{012d}', 'I', '\u{0131}',
    '\u{014e}', '\u{014f}',
];

const WP6_CS2: &[char] = &[
    '\u{02b9}', '\u{02ba}', '\u{02bb}', ' ', '\u{02bd}', '\u{02bc}', ' ', '\u{02be}', '\u{02bf}',
    '\u{0310}', '\u{02d0}', '\u{02d1}', '\u{0306}', '\u{032e}', '\u{0329}', '\u{02c8}', '\u{02cc}',
    '\u{02c9}', '\u{02ca}', '\u{02cb}', '\u{02cd}', '\u{02ce}', '\u{02cf}', '\u{02c6}', '\u{02c7}',
    '\u{02dc}', '\u{0325}', '\u{02da}', '\u{032d}', '\u{032c}', '\u{0323}', '\u{0308}', '\u{0324}',
    '\u{031c}', '\u{031d}', '\u{031e}', '\u{031f}', '\u{0320}', '\u{0321}', '\u{0322}', '\u{032a}',
    '\u{032b}', '\u{02d2}', '\u{02d3}', '\u{0361}', '\u{0356}', '_', '\u{2017}', '\u{033e}',
    '\u{02db}', '\u{0327}', '\u{0233}', '\u{030d}', '\u{02b0}', '\u{02b6}', '\u{0250}', '\u{0251}',
    '\u{0252}', '\u{0253}', '\u{0299}', '\u{0254}', '\u{0255}', '\u{0297}', '\u{0256}', '\u{0257}',
    '\u{0258}', '\u{0259}', '\u{025a}', '\u{025b}', '\u{025c}', '\u{025d}', '\u{029a}', '\u{025e}',
    '\u{025f}', '\u{0278}', '\u{0261}', '\u{0260}', '\u{0262}', '\u{029b}', '\u{0263}', '\u{0264}',
    '\u{0265}', '\u{0266}', '\u{0267}', '\u{029c}', '\u{0268}', '\u{026a}', '\u{0269}', '\u{029d}',
    '\u{029e}', '\u{026b}', '\u{026c}', '\u{026d}', '\u{029f}', '\u{026e}', '\u{028e}', '\u{026f}',
    '\u{0270}', '\u{0271}', '\u{0272}', '\u{0273}', '\u{0274}', '\u{0276}', '\u{0277}', '\u{02a0}',
    '\u{0279}', '\u{027a}', '\u{027b}', '\u{027c}', '\u{027d}', '\u{027e}', '\u{027f}', '\u{0280}',
    '\u{0281}', '\u{0282}', '\u{0283}', '\u{0284}', '\u{0285}', '\u{0286}', '\u{0287}', '\u{0288}',
    '\u{0275}', '\u{0289}', '\u{028a}', '\u{028c}', '\u{028b}', '\u{028d}', '\u{03c7}', '\u{028f}',
    '\u{0290}', '\u{0291}', '\u{0292}', '\u{0293}', '\u{0294}', '\u{0295}', '\u{0296}', '\u{02a1}',
    '\u{02a2}', '\u{0298}', '\u{02a3}', '\u{02a4}', '\u{02a5}', '\u{02a6}', '\u{02a7}', '\u{02a8}',
];

const WP6_CS3: &[char] = &[
    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2588}', '\u{258c}', '\u{2580}', '\u{2590}', '\u{2584}',
    '\u{2500}', '\u{2502}', '\u{250c}', '\u{2510}', '\u{2518}', '\u{2514}', '\u{251c}', '\u{252c}',
    '\u{2524}', '\u{2534}', '\u{253c}', '\u{2550}', '\u{2551}', '\u{2554}', '\u{2557}', '\u{255d}',
    '\u{255a}', '\u{2560}', '\u{2566}', '\u{2563}', '\u{2569}', '\u{256c}', '\u{2552}', '\u{2555}',
    '\u{255b}', '\u{2558}', '\u{2553}', '\u{2556}', '\u{255c}', '\u{2559}', '\u{255e}', '\u{2565}',
    '\u{2561}', '\u{2568}', '\u{255f}', '\u{2564}', '\u{2562}', '\u{2567}', '\u{256b}', '\u{256a}',
    '\u{2574}', '\u{2575}', '\u{2576}', '\u{2577}', '\u{2578}', '\u{2579}', '\u{257a}', '\u{257b}',
    '\u{257c}', '\u{257e}', '\u{257d}', '\u{257f}', '\u{251f}', '\u{2522}', '\u{251e}', '\u{2521}',
    '\u{252e}', '\u{2532}', '\u{252d}', '\u{2531}', '\u{2527}', '\u{2526}', '\u{252a}', '\u{2529}',
    '\u{2536}', '\u{253a}', '\u{2535}', '\u{2539}', '\u{2541}', '\u{2546}', '\u{253e}', '\u{2540}',
    '\u{2544}', '\u{254a}', '\u{253d}', '\u{2545}', '\u{2548}', '\u{2543}', '\u{2549}', '\u{2547}',
];

const WP6_CS4: &[char] = &[
    '\u{25cf}', '\u{25cb}', '\u{25a0}', '\u{2022}', '*', '\u{00b6}', '\u{00a7}', '\u{00a1}',
    '\u{00bf}', '\u{00ab}', '\u{00bb}', '\u{00a3}', '\u{00a5}', '\u{20a7}', '\u{0192}', '\u{00aa}',
    '\u{00ba}', '\u{00bd}', '\u{00bc}', '\u{00a2}', '\u{00b2}', '\u{207f}', '\u{00ae}', '\u{00a9}',
    '\u{00a4}', '\u{00be}', '\u{00b3}', '\u{201b}', '\u{2019}', '\u{2018}', '\u{201f}', '\u{201d}',
    '\u{201c}', '\u{2013}', '\u{2014}', '\u{2039}', '\u{203a}', '\u{25cb}', '\u{25a1}', '\u{2020}',
    '\u{2021}', '\u{2122}', '\u{2120}', '\u{211e}', '\u{25cf}', '\u{25e6}', '\u{25a0}', '\u{25aa}',
    '\u{25a1}', '\u{25ab}', '\u{2012}', '\u{fb00}', '\u{fb03}', '\u{fb04}', '\u{fb01}', '\u{fb02}',
    '\u{2026}', '$', '\u{20a3}', '\u{20a2}', '\u{20a0}', '\u{20a4}', '\u{201a}', '\u{201e}',
    '\u{2153}', '\u{2154}', '\u{215b}', '\u{215c}', '\u{215d}', '\u{215e}', '\u{24c2}', '\u{24c5}',
    '\u{20ac}', '\u{2105}', '\u{2106}', '\u{2030}', '\u{2116}', '\u{2014}', '\u{00b9}', '\u{2409}',
    '\u{240c}', '\u{240d}', '\u{240a}', '\u{2424}', '\u{240b}', '\u{267c}', '\u{20a9}', '\u{20a6}',
    '\u{20a8}', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', '\u{1d11}', '\u{1d12}',
];

const WP6_CS5: &[char] = &[
    '\u{2661}', '\u{2662}', '\u{2667}', '\u{2664}', '\u{2642}', '\u{2640}', '\u{263c}', '\u{263a}',
    '\u{263b}', '\u{266a}', '\u{266c}', '\u{25ac}', '\u{2302}', '\u{203c}', '\u{221a}', '\u{21a8}',
    '\u{2310}', '\u{2319}', '\u{25d8}', '\u{25d9}', '\u{21b5}', '\u{2104}', '\u{261c}', '\u{23b5}',
    '\u{2610}', '\u{2612}', '\u{2639}', '\u{266f}', '\u{266d}', '\u{266e}', '\u{260e}', '\u{231a}',
    '\u{231b}', '\u{2701}', '\u{2702}', '\u{2703}', '\u{2704}', '\u{260e}', '\u{2706}', '\u{2707}',
    '\u{2708}', '\u{2709}', '\u{261b}', '\u{261e}', '\u{270c}', '\u{270d}', '\u{270e}', '\u{270f}',
    '\u{2710}', '\u{2711}', '\u{2712}', '\u{2713}', '\u{2714}', '\u{2715}', '\u{2716}', '\u{2717}',
    '\u{2718}', '\u{2719}', '\u{271a}', '\u{271b}', '\u{271c}', '\u{271d}', '\u{271e}', '\u{271f}',
    '\u{2720}', '\u{2721}', '\u{2722}', '\u{2723}', '\u{2724}', '\u{2725}', '\u{2726}', '\u{2727}',
    '\u{2605}', '\u{2606}', '\u{272a}', '\u{272b}', '\u{272c}', '\u{272d}', '\u{272e}', '\u{272f}',
    '\u{2730}', '\u{2731}', '\u{2732}', '\u{2733}', '\u{2734}', '\u{2735}', '\u{2736}', '\u{2737}',
    '\u{2738}', '\u{2739}', '\u{273a}', '\u{273b}', '\u{273c}', '\u{273d}', '\u{273e}', '\u{273f}',
    '\u{2740}', '\u{2741}', '\u{2742}', '\u{2743}', '\u{2744}', '\u{2745}', '\u{2746}', '\u{2747}',
    '\u{2748}', '\u{2749}', '\u{274a}', '\u{274b}', '\u{25cf}', '\u{274d}', '\u{25a0}', '\u{274f}',
    '\u{2750}', '\u{2751}', '\u{2752}', '\u{25b2}', '\u{25bc}', '\u{25c6}', '\u{2756}', '\u{25d7}',
    '\u{2758}', '\u{2759}', '\u{275a}', '\u{275b}', '\u{275c}', '\u{275d}', '\u{275e}', '\u{2036}',
    '\u{2033}', ' ', ' ', ' ', ' ', '\u{2329}', '\u{232a}', '[', ']', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', '\u{2190}', ' ', ' ', ' ', ' ', ' ', '\u{21e8}', '\u{21e6}', '\u{2794}', ' ', ' ', ' ',
    ' ', ' ', '\u{25d6}', ' ', ' ', '\u{2761}', '\u{2762}', '\u{2763}', '\u{2764}', '\u{2765}',
    '\u{2766}', '\u{2767}', '\u{2663}', '\u{2666}', '\u{2665}', '\u{2660}', '\u{2780}', '\u{2781}',
    '\u{2782}', '\u{2783}', '\u{2784}', '\u{2785}', '\u{2786}', '\u{2787}', '\u{2788}', '\u{2789}',
    '\u{2776}', '\u{2777}', '\u{2778}', '\u{2779}', '\u{277a}', '\u{277b}', '\u{277c}', '\u{277d}',
    '\u{277e}', '\u{277f}', '\u{2780}', '\u{2781}', '\u{2782}', '\u{2783}', '\u{2784}', '\u{2785}',
    '\u{2786}', '\u{2787}', '\u{2788}', '\u{2789}', '\u{278a}', '\u{278b}', '\u{278c}', '\u{278d}',
    '\u{278e}', '\u{278f}', '\u{2790}', '\u{2791}', '\u{2792}', '\u{2793}', '\u{2794}', '\u{2192}',
    '\u{2194}', '\u{2195}', '\u{2798}', '\u{2799}', '\u{279a}', '\u{279b}', '\u{279c}', '\u{279d}',
    '\u{279e}', '\u{279f}', '\u{27a0}', '\u{27a1}', '\u{27a2}', '\u{27a3}', '\u{27a4}', '\u{27a5}',
    '\u{27a6}', '\u{27a7}', '\u{27a8}', '\u{27a9}', '\u{27aa}', '\u{27ab}', '\u{27ac}', '\u{27ad}',
    '\u{27ae}', '\u{27af}', ' ', '\u{27b1}', '\u{27b2}', '\u{27b3}', '\u{27b4}', '\u{27b5}',
    '\u{27b6}', '\u{27b7}', '\u{27b8}', '\u{27b9}', '\u{27ba}', '\u{27bb}', '\u{27bc}', '\u{27bd}',
    '\u{27be}',
];

const WP6_CS6: &[char] = &[
    '\u{2212}', '\u{00b1}', '\u{2264}', '\u{2265}', '\u{221d}', '/', '\u{2215}', '\u{2216}',
    '\u{00f7}', '\u{2223}', '\u{27e8}', '\u{27e9}', '\u{223c}', '\u{2248}', '\u{2261}', '\u{2208}',
    '\u{2229}', '\u{2225}', '\u{2211}', '\u{221e}', '\u{00ac}', '\u{2192}', '\u{2190}', '\u{2191}',
    '\u{2193}', '\u{2194}', '\u{2195}', '\u{25b8}', '\u{25c2}', '\u{25b4}', '\u{25be}', '\u{22c5}',
    '\u{00b7}', '\u{2218}', '\u{2219}', '\u{212b}', '\u{00b0}', '\u{00b5}', '\u{203e}', '\u{00d7}',
    '\u{222b}', '\u{220f}', '\u{2213}', '\u{2207}', '\u{2202}', '\u{2032}', '\u{2033}', '\u{2192}',
    '\u{212f}', '\u{2113}', '\u{210f}', '\u{2111}', '\u{211c}', '\u{2118}', '\u{21c4}', '\u{21c6}',
    '\u{21d2}', '\u{21d0}', '\u{21d1}', '\u{21d3}', '\u{21d4}', '\u{21d5}', '\u{2197}', '\u{2198}',
    '\u{2196}', '\u{2199}', '\u{222a}', '\u{2282}', '\u{2283}', '\u{2286}', '\u{2287}', '\u{220d}',
    '\u{2205}', '\u{2308}', '\u{2309}', '\u{230a}', '\u{230b}', '\u{226a}', '\u{226b}', '\u{2220}',
    '\u{2297}', '\u{2295}', '\u{2296}', '\u{2a38}', '\u{2299}', '\u{2227}', '\u{2228}', '\u{22bb}',
    '\u{22a4}', '\u{22a5}', '\u{2312}', '\u{22a2}', '\u{22a3}', '\u{25a1}', '\u{25a0}', '\u{25ca}',
    '\u{25c6}', '\u{27e6}', '\u{27e7}', '\u{2260}', '\u{2262}', '\u{2235}', '\u{2234}', '\u{2237}',
    '\u{222e}', '\u{2112}', '\u{212d}', '\u{2128}', '\u{2118}', '\u{20dd}', '\u{29cb}', '\u{25c7}',
    '\u{22c6}', '\u{2034}', '\u{2210}', '\u{2243}', '\u{2245}', '\u{227a}', '\u{227c}', '\u{227b}',
    '\u{227d}', '\u{2203}', '\u{2200}', '\u{22d8}', '\u{22d9}', '\u{228e}', '\u{228a}', '\u{228b}',
    '\u{2293}', '\u{2294}', '\u{228f}', '\u{2291}', '\u{22e4}', '\u{2290}', '\u{2292}', '\u{22e5}',
    '\u{25b3}', '\u{25bd}', '\u{25c3}', '\u{25b9}', '\u{22c8}', '\u{2323}', '\u{2322}', '\u{25ef}',
    '\u{219d}', '\u{21a9}', '\u{21aa}', '\u{21a3}', '\u{21bc}', '\u{21bd}', '\u{21c0}', '\u{21c1}',
    '\u{21cc}', '\u{21cb}', '\u{21bf}', '\u{21be}', '\u{21c3}', '\u{21c2}', '\u{21c9}', '\u{21c7}',
    '\u{22d3}', '\u{22d2}', '\u{22d0}', '\u{22d1}', '\u{229a}', '\u{229b}', '\u{229d}', '\u{2127}',
    '\u{2221}', '\u{2222}', '\u{25c3}', '\u{25b9}', '\u{25b5}', '\u{25bf}', '\u{2214}', '\u{2250}',
    '\u{2252}', '\u{2253}', '\u{224e}', '\u{224d}', '\u{22a8}', '\u{2258}', '\u{226c}', '\u{0285}',
    '\u{2605}', '\u{226e}', '\u{2270}', '\u{226f}', '\u{2271}', '\u{2241}', '\u{2244}', '\u{2247}',
    '\u{2249}', '\u{2280}', '\u{22e0}', '\u{2281}', '\u{22e1}', '\u{2284}', '\u{2285}', '\u{2288}',
    '\u{2289}', ' ', ' ', '\u{22e2}', '\u{22e3}', '\u{2226}', '\u{2224}', '\u{226d}', '\u{2204}',
    '\u{2209}', '\u{2247}', '\u{2130}', '\u{2131}', '\u{2102}', ' ', '\u{2115}', '\u{211d}',
    '\u{225f}', '\u{22be}', '\u{220b}', '\u{22ef}', '\u{2026}', '\u{22ee}', '\u{22f1}', ' ',
    '\u{20e1}', '+', '-', '=', '*', '\u{2032}', '\u{2033}', '\u{2034}', '\u{210b}', '\u{2118}',
    '\u{2272}', '\u{2273}', ' ',
];

const WP6_CS7: &[char] = &[
    '\u{2320}', '\u{2321}', '\u{23a5}', '\u{23bd}', '\u{221a}', ' ', '\u{2211}', '\u{220f}',
    '\u{2210}', '\u{222b}', '\u{222e}', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', '\u{23a7}', '\u{23a8}', '\u{23a9}', '\u{23aa}', ' ', ' ', ' ', ' ', '\u{23ab}',
    '\u{23ac}', '\u{23ad}', '\u{23aa}', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', '\u{222a}', '\u{222b}', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', '\u{239b}', '\u{239d}', '\u{239c}', ' ', ' ', ' ', ' ', '\u{239e}', '\u{23a8}',
    '\u{239f}', ' ', ' ', ' ', ' ', '\u{23a1}', '\u{23a3}', '\u{23a2}', ' ', '\u{20aa}', ' ', ' ',
    '\u{23a4}', '\u{23a6}', '\u{23a5}', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', '\u{22c3}', '\u{22c2}', '\u{228e}', '\u{2a04}', '\u{2294}', '\u{2a06}',
    '\u{2227}', '\u{22c0}', '\u{2228}', '\u{22c1}', '\u{2297}', '\u{2a02}', '\u{2295}', '\u{2a01}',
    '\u{2299}', '\u{2a00}', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', '\u{229d}', ' ', '\u{2238}', ' ', '\u{27e6}', ' ', ' ', ' ', ' ',
    ' ', ' ', '\u{27e7}', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', '\u{21bc}', '\u{21bd}', ' ',
    '\u{296c}', '\u{296d}', '\u{296a}', '\u{296b}', ' ', '\u{21c9}', '\u{21c7}', ' ', ' ', ' ',
    ' ', ' ', ' ', '\u{21be}', '\u{21bf}', '\u{21c3}', '\u{21c2}', ' ', '\u{2293}', '\u{2a05}',
    '\u{23a1}', ' ', ' ', ' ', ' ', ' ',
];

const WP6_CS8: &[char] = &[
    '\u{0391}', '\u{03b1}', '\u{0392}', '\u{03b2}', '\u{0392}', '\u{03d0}', '\u{0393}', '\u{03b3}',
    '\u{0394}', '\u{03b4}', '\u{0395}', '\u{03b5}', '\u{0396}', '\u{03b6}', '\u{0397}', '\u{03b7}',
    '\u{0398}', '\u{03b8}', '\u{0399}', '\u{03b9}', '\u{039a}', '\u{03ba}', '\u{039b}', '\u{03bb}',
    '\u{039c}', '\u{03bc}', '\u{039d}', '\u{03bd}', '\u{039e}', '\u{03be}', '\u{039f}', '\u{03bf}',
    '\u{03a0}', '\u{03c0}', '\u{03a1}', '\u{03c1}', '\u{03a3}', '\u{03c3}', '\u{03a3}', '\u{03c2}',
    '\u{03a4}', '\u{03c4}', '\u{03a5}', '\u{03c5}', '\u{03a6}', '\u{03c6}', '\u{03a7}', '\u{03c7}',
    '\u{03a8}', '\u{03c8}', '\u{03a9}', '\u{03c9}', '\u{0386}', '\u{03ac}', '\u{0388}', '\u{03ad}',
    '\u{0389}', '\u{03ae}', '\u{038a}', '\u{03af}', '\u{03aa}', '\u{03ca}', '\u{038c}', '\u{03cc}',
    '\u{038e}', '\u{03cd}', '\u{03ab}', '\u{03cb}', '\u{038f}', '\u{03ce}', '\u{03b5}', '\u{03d1}',
    '\u{03f0}', '\u{03d6}', '\u{03f1}', '\u{03c2}', '\u{03d2}', '\u{03d5}', '\u{03c9}', '\u{037e}',
    '\u{0387}', '\u{0374}', '\u{0375}', '\u{0384}', '\u{00a8}', '\u{0385}', '\u{1fed}', '\u{1fef}',
    '\u{1fc0}', '\u{1fbd}', '\u{1ffe}', '\u{037a}', '\u{1fce}', '\u{1fde}', '\u{1fcd}', '\u{1fdd}',
    '\u{1fcf}', '\u{1fdf}', '\u{0384}', '\u{1fef}', '\u{1fc0}', '\u{1fbd}', '\u{1ffe}', '\u{1fce}',
    '\u{1fde}', '\u{1fcd}', '\u{1fdd}', '\u{1fcf}', '\u{1fdf}', '\u{1f70}', '\u{1fb6}', '\u{1fb3}',
    '\u{1fb4}', '\u{1fb2}', '\u{1fb7}', '\u{1f00}', '\u{1f04}', '\u{1f02}', '\u{1f06}', '\u{1f80}',
    '\u{1f84}', '\u{1f82}', '\u{1f86}', '\u{1f01}', '\u{1f05}', '\u{1f03}', '\u{1f07}', '\u{1f81}',
    '\u{1f85}', '\u{1f83}', '\u{1f87}', '\u{1f72}', '\u{1f10}', '\u{1f14}', '\u{1f12}', '\u{1f11}',
    '\u{1f15}', '\u{1f13}', '\u{1f74}', '\u{1fc6}', '\u{1fc3}', '\u{1fc4}', '\u{1fc2}', '\u{1fc7}',
    '\u{1f20}', '\u{1f24}', '\u{1f22}', '\u{1f26}', '\u{1f90}', '\u{1f94}', '\u{1f92}', '\u{1f96}',
    '\u{1f21}', '\u{1f25}', '\u{1f23}', '\u{1f27}', '\u{1f91}', '\u{1f95}', '\u{1f93}', '\u{1f97}',
    '\u{1f76}', '\u{1fd6}', '\u{0390}', '\u{1fd2}', '\u{1f30}', '\u{1f34}', '\u{1f32}', '\u{1f36}',
    '\u{1f31}', '\u{1f35}', '\u{1f33}', '\u{1f37}', '\u{1f78}', '\u{1f40}', '\u{1f44}', '\u{1f42}',
    '\u{1f41}', '\u{1f45}', '\u{1f43}', '\u{1fe5}', '\u{1fe4}', '\u{1f7a}', '\u{1fe6}', '\u{03b0}',
    '\u{1fe2}', '\u{1f50}', '\u{1f54}', '\u{1f52}', '\u{1f56}', '\u{1f51}', '\u{1f55}', '\u{1f53}',
    '\u{1f57}', '\u{1f7c}', '\u{1ff6}', '\u{1ff3}', '\u{1ff4}', '\u{1ff2}', '\u{1ff7}', '\u{1f60}',
    '\u{1f64}', '\u{1f62}', '\u{1f66}', '\u{1fa0}', '\u{1fa4}', '\u{1fa2}', '\u{1fa6}', '\u{1f61}',
    '\u{1f65}', '\u{1f63}', '\u{1f67}', '\u{1fa1}', '\u{1fa5}', '\u{1fa3}', '\u{1fa7}', '\u{03da}',
    '\u{03dc}', '\u{03de}', '\u{03e0}',
];

const WP6_CS9: &[char] = &[
    '\u{05d0}', '\u{05d1}', '\u{05d2}', '\u{05d3}', '\u{05d4}', '\u{05d5}', '\u{05d6}', '\u{05d7}',
    '\u{05d8}', '\u{05d9}', '\u{05da}', '\u{05db}', '\u{05dc}', '\u{05dd}', '\u{05de}', '\u{05df}',
    '\u{05e0}', '\u{05e1}', '\u{05e2}', '\u{05e3}', '\u{05e4}', '\u{05e5}', '\u{05e6}', '\u{05e7}',
    '\u{05e8}', '\u{05e9}', '\u{05ea}', '\u{05be}', '\u{05c0}', '\u{05c3}', '\u{05f3}', '\u{05f4}',
    '\u{05b0}', '\u{05b1}', '\u{05b2}', '\u{05b3}', '\u{05b4}', '\u{05b5}', '\u{05b6}', '\u{05b7}',
    '\u{05b8}', '\u{05b9}', '\u{05b9}', '\u{05bb}', '\u{05bc}', '\u{05bd}', '\u{05bf}', '\u{05b7}',
    '\u{fb1e}', '\u{05f0}', '\u{05f1}', '\u{05f2}', '\u{fb1f}', '\u{0591}', '\u{0596}', ' ',
    '\u{05a4}', '\u{059a}', '\u{059b}', '\u{05a3}', '\u{05a5}', '\u{05a6}', '\u{05a7}', '\u{05a2}',
    '\u{0592}', '\u{0593}', '\u{0594}', '\u{0595}', '\u{0597}', '\u{0598}', '\u{0599}', '\u{05a8}',
    '\u{05f3}', '\u{05f3}', '\u{05f4}', ' ', '\u{05a9}', '\u{05a0}', '\u{059f}', '\u{05ab}',
    '\u{05ac}', '\u{05af}', '\u{05c4}', '\u{05aa}', '\u{fb30}', '\u{fb31}', '\u{05d1}', '\u{fb32}',
    '\u{fb33}', '\u{fb34}', '\u{fb35}', '\u{fb4b}', '\u{fb36}', '\u{05d7}', '\u{fb38}', '\u{fb39}',
    '\u{fb1d}', '\u{fb3b}', '\u{fb3a}', '\u{05da}', '\u{05da}', '\u{05da}', '\u{05da}', '\u{05da}',
    '\u{05da}', '\u{fb3c}', '\u{fb3e}', '\u{fb40}', '\u{05d5}', '\u{fb41}', '\u{fb44}', '\u{05e4}',
    '\u{fb46}', '\u{fb47}', '\u{fb2b}', '\u{fb2d}', '\u{fb2a}', '\u{fb2c}', '\u{fb4a}', '\u{05dc}',
    '\u{fb3c}', '\u{fb49}', '\u{20aa}',
];

const WP6_CS10: &[char] = &[
    '\u{0410}', '\u{0430}', '\u{0411}', '\u{0431}', '\u{0412}', '\u{0432}', '\u{0413}', '\u{0433}',
    '\u{0414}', '\u{0434}', '\u{0415}', '\u{0435}', '\u{0401}', '\u{0451}', '\u{0416}', '\u{0436}',
    '\u{0417}', '\u{0437}', '\u{0418}', '\u{0438}', '\u{0419}', '\u{0439}', '\u{041a}', '\u{043a}',
    '\u{041b}', '\u{043b}', '\u{041c}', '\u{043c}', '\u{041d}', '\u{043d}', '\u{041e}', '\u{043e}',
    '\u{041f}', '\u{043f}', '\u{0420}', '\u{0440}', '\u{0421}', '\u{0441}', '\u{0422}', '\u{0442}',
    '\u{0423}', '\u{0443}', '\u{0424}', '\u{0444}', '\u{0425}', '\u{0445}', '\u{0426}', '\u{0446}',
    '\u{0427}', '\u{0447}', '\u{0428}', '\u{0448}', '\u{0429}', '\u{0449}', '\u{042a}', '\u{044a}',
    '\u{042b}', '\u{044b}', '\u{042c}', '\u{044c}', '\u{042d}', '\u{044d}', '\u{042e}', '\u{044e}',
    '\u{042f}', '\u{044f}', '\u{04d8}', '\u{04d9}', '\u{0403}', '\u{0453}', '\u{0490}', '\u{0491}',
    '\u{0492}', '\u{0493}', '\u{0402}', '\u{0452}', '\u{0404}', '\u{0454}', '\u{0404}', '\u{0454}',
    '\u{0496}', '\u{0497}', '\u{0405}', '\u{0455}', ' ', ' ', '\u{0418}', '\u{0438}', '\u{0406}',
    '\u{0456}', '\u{0407}', '\u{0457}', ' ', ' ', '\u{0408}', '\u{0458}', '\u{040c}', '\u{045c}',
    '\u{049a}', '\u{049b}', '\u{04c3}', '\u{04c4}', '\u{049c}', '\u{049d}', '\u{0409}', '\u{0459}',
    '\u{04a2}', '\u{04a3}', '\u{040a}', '\u{045a}', '\u{047a}', '\u{047b}', '\u{0460}', '\u{0461}',
    '\u{040b}', '\u{045b}', '\u{040e}', '\u{045e}', '\u{04ee}', '\u{04ef}', '\u{04ae}', '\u{04af}',
    '\u{04b0}', '\u{04b1}', '\u{0194}', '\u{0263}', '\u{04b2}', '\u{04b3}', '\u{0425}', '\u{0445}',
    '\u{04ba}', '\u{04bb}', '\u{047e}', '\u{047f}', '\u{040f}', '\u{045f}', '\u{04b6}', '\u{04b7}',
    '\u{04b8}', '\u{04b9}', '\u{0428}', '\u{0448}', '\u{0462}', '\u{0463}', '\u{0466}', '\u{0467}',
    '\u{046a}', '\u{046b}', '\u{046e}', '\u{046f}', '\u{0470}', '\u{0471}', '\u{0472}', '\u{0473}',
    '\u{0474}', '\u{0475}', '\u{0410}', '\u{0430}', '\u{0415}', '\u{0435}', '\u{0404}', '\u{0454}',
    '\u{0418}', '\u{0438}', '\u{0406}', '\u{0456}', '\u{0407}', '\u{0457}', '\u{041e}', '\u{043e}',
    '\u{0423}', '\u{0443}', '\u{042b}', '\u{044b}', '\u{042d}', '\u{044d}', '\u{042e}', '\u{044e}',
    '\u{042f}', '\u{044f}', '\u{0410}', '\u{0430}', '\u{0400}', '\u{0450}', '\u{0401}', '\u{0451}',
    '\u{040d}', '\u{045d}', '\u{041e}', '\u{043e}', '\u{0423}', '\u{0443}', '\u{042b}', '\u{044b}',
    '\u{042d}', '\u{044d}', '\u{042e}', '\u{044e}', '\u{042f}', '\u{044f}', '\u{0301}', '\u{0300}',
    '\u{0308}', '\u{0306}', '\u{0326}', '\u{0328}', '\u{0304}', ' ', '\u{201e}', '\u{201c}',
    '\u{10d0}', '\u{10d1}', '\u{10d2}', '\u{10d3}', '\u{10d4}', '\u{10d5}', '\u{10d6}', '\u{10f1}',
    '\u{10d7}', '\u{10d8}', '\u{10d9}', '\u{10da}', '\u{10db}', '\u{10dc}', '\u{10f2}', '\u{10dd}',
    '\u{10de}', '\u{10df}', '\u{10e0}', '\u{10e1}', '\u{10e2}', '\u{10e3}', '\u{10f3}', '\u{10e4}',
    '\u{10e5}', '\u{10e6}', '\u{10e7}', '\u{10e8}', '\u{10e9}', '\u{10ea}', '\u{10eb}', '\u{10ec}',
    '\u{10ed}', '\u{10ee}', '\u{10f4}', '\u{10ef}', '\u{10f0}', '\u{10f5}', '\u{10f6}', '\u{10e3}',
];

const WP6_CS11: &[char] = &[
    '\u{ff61}', '\u{ff62}', '\u{ff63}', '\u{ff64}', '\u{ff65}', '\u{ff66}', '\u{ff67}', '\u{ff68}',
    '\u{ff69}', '\u{ff6a}', '\u{ff6b}', '\u{ff6c}', '\u{ff6d}', '\u{ff6e}', '\u{ff6f}', '\u{ff70}',
    '\u{ff71}', '\u{ff72}', '\u{ff73}', '\u{ff74}', '\u{ff75}', '\u{ff76}', '\u{ff77}', '\u{ff78}',
    '\u{ff79}', '\u{ff7a}', '\u{ff7b}', '\u{ff7c}', '\u{ff7d}', '\u{ff7e}', '\u{ff7f}', '\u{ff80}',
    '\u{ff81}', '\u{ff82}', '\u{ff83}', '\u{ff84}', '\u{ff85}', '\u{ff86}', '\u{ff87}', '\u{ff88}',
    '\u{ff89}', '\u{ff8a}', '\u{ff8b}', '\u{ff8c}', '\u{ff8d}', '\u{ff8e}', '\u{ff8f}', '\u{ff90}',
    '\u{ff91}', '\u{ff92}', '\u{ff93}', '\u{ff94}', '\u{ff95}', '\u{ff96}', '\u{ff97}', '\u{ff98}',
    '\u{ff99}', '\u{ff9a}', '\u{ff9b}', '\u{ff9c}', '\u{ff9d}', '\u{ff9e}', '\u{ff9f}',
];

const WP6_CS12: &[char] = &[
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
];

const WP6_CS13: &[char] = &[
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', '\u{064e}', '\u{fe77}', '\u{064f}',
    '\u{fe79}', '\u{0650}', '\u{fe7b}', '\u{064b}', '\u{064c}', '\u{064c}', '\u{064d}', '\u{0652}',
    '\u{fe7f}', '\u{0651}', '\u{fe7d}', '\u{fc60}', '\u{fcf2}', '\u{fc61}', '\u{fcf3}', '\u{fc62}',
    '\u{fcf4}', '\u{064b}', '\u{fc5e}', '\u{fc5e}', '\u{fc5f}', '\u{0653}', '\u{0670}', '\u{0654}',
    ' ', '\u{060c}', '\u{061b}', '\u{061f}', '\u{066d}', '\u{066a}', '\u{00bb}', '\u{00ab}', ')',
    '(', '\u{0661}', '\u{0662}', '\u{0663}', '\u{0664}', '\u{0665}', '\u{0666}', '\u{0667}',
    '\u{0668}', '\u{0669}', '\u{0660}', '\u{0662}', '\u{0627}', '\u{fe8e}', '\u{0628}', '\u{fe91}',
    '\u{fe92}', '\u{fe90}', '\u{062a}', '\u{fe97}', '\u{fe98}', '\u{fe96}', '\u{062b}', '\u{fe9b}',
    '\u{fe9c}', '\u{fe9a}', '\u{062c}', '\u{fe9f}', '\u{fea0}', '\u{fe9e}', '\u{062d}', '\u{fea3}',
    '\u{fea4}', '\u{fea2}', '\u{062e}', '\u{fea7}', '\u{fea8}', '\u{fea6}', '\u{062f}', '\u{feaa}',
    '\u{0630}', '\u{feac}', '\u{0631}', '\u{feae}', '\u{0632}', '\u{feaf}', '\u{0633}', '\u{feb3}',
    '\u{feb4}', '\u{feb2}', '\u{0634}', '\u{feb7}', '\u{feb8}', '\u{feb6}', '\u{0635}', '\u{febb}',
    '\u{febc}', '\u{feba}', '\u{0636}', '\u{febf}', '\u{fec0}', '\u{febe}', '\u{0637}', '\u{fec3}',
    '\u{fec4}', '\u{fec2}', '\u{0638}', '\u{fec7}', '\u{fec8}', '\u{fec6}', '\u{0639}', '\u{fecb}',
    '\u{fecc}', '\u{feca}', '\u{063a}', '\u{fecf}', '\u{fed0}', '\u{fece}', '\u{0641}', '\u{fed3}',
    '\u{fed4}', '\u{fed2}', '\u{0642}', '\u{fed7}', '\u{fed8}', '\u{fed6}', '\u{0643}', '\u{fedb}',
    '\u{fedc}', '\u{feda}', '\u{0644}', '\u{fedf}', '\u{fee0}', '\u{fede}', '\u{0645}', '\u{fee3}',
    '\u{fee4}', '\u{fee2}', '\u{0646}', '\u{fee7}', '\u{fee8}', '\u{fee6}', '\u{0647}', '\u{feeb}',
    '\u{feec}', '\u{feea}', '\u{0629}', '\u{fe94}', '\u{0648}', '\u{feee}', '\u{064a}', '\u{fef3}',
    '\u{fef4}', '\u{fef2}', '\u{0649}', '\u{fef3}', '\u{fef4}', '\u{fef0}', '\u{0621}', '\u{0623}',
    '\u{fe84}', '\u{0625}', '\u{fe88}', '\u{0624}', '\u{fe86}', '\u{0626}', '\u{fe8b}', '\u{fe8c}',
    '\u{fe8a}', '\u{fd3d}', '\u{fd3c}', '\u{0622}', '\u{fe82}', '\u{0671}', '\u{fb51}', '\u{fefb}',
    '\u{fefc}', '\u{fef7}', '\u{fef8}', '\u{fef9}', '\u{fefa}', ' ', '\u{fefc}', '\u{fef5}',
    '\u{fef6}', ' ', ' ', '\u{fdf2}', '\u{0640}', '\u{0640}',
];

const WP6_CS14: &[char] = &[
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', '\u{0615}', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    '\u{0615}', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', '\u{06d4}', ' ', ' ', '\u{00b0}', ' ',
    '\u{065a}', ' ', '\u{065a}', '\u{0659}', ' ', ' ', '\u{0654}', '\u{064c}', '\u{fc5e}',
    '\u{065a}', '\u{065a}', '\u{06f4}', '\u{06f4}', '\u{06f5}', '\u{06f6}', '\u{06f6}', '\u{06f7}',
    '\u{06f8}', '\u{067b}', '\u{fb54}', '\u{fb55}', '\u{fb53}', '\u{0680}', '\u{fb5c}', '\u{fb5d}',
    '\u{fb5b}', '\u{067e}', '\u{fb58}', '\u{fb59}', '\u{fb57}', '\u{0679}', '\u{fb68}', '\u{fb69}',
    '\u{fb67}', '\u{067c}', '\u{067c}', '\u{067c}', '\u{067c}', '\u{067f}', '\u{fb64}', '\u{fb65}',
    '\u{fb63}', '\u{067d}', '\u{067d}', '\u{067d}', '\u{067d}', '\u{067a}', '\u{fb60}', '\u{fb61}',
    '\u{fb5f}', '\u{0684}', '\u{fb74}', '\u{fb75}', '\u{fb73}', '\u{0683}', '\u{fb78}', '\u{fb79}',
    '\u{fb77}', '\u{0686}', '\u{fb7c}', '\u{fb7d}', '\u{fb7b}', '\u{0687}', '\u{fb80}', '\u{fb81}',
    '\u{fb7f}', '\u{0685}', '\u{0685}', '\u{0685}', '\u{0685}', '\u{0681}', '\u{0681}', '\u{0681}',
    '\u{0681}', '\u{0688}', '\u{fb89}', '\u{0689}', '\u{0689}', '\u{068c}', '\u{fb85}', '\u{068e}',
    '\u{fb87}', '\u{068a}', '\u{068a}', '\u{068d}', '\u{fb83}', '\u{0693}', '\u{0693}', '\u{0691}',
    '\u{fb8d}', '\u{0699}', '\u{0699}', '\u{0695}', '\u{0695}', '\u{0692}', '\u{0692}', '\u{0698}',
    '\u{fb8b}', '\u{0696}', '\u{0696}', '\u{075b}', '\u{075b}', '\u{069a}', '\u{069a}', '\u{069a}',
    '\u{069a}', '\u{06a0}', '\u{06a0}', '\u{06a0}', '\u{06a0}', '\u{06a4}', '\u{fb6c}', '\u{fb6d}',
    '\u{fb6b}', '\u{06a6}', '\u{fb70}', '\u{fb71}', '\u{fb6f}', '\u{06a9}', '\u{fb90}', '\u{fb91}',
    '\u{fb8f}', '\u{0643}', '\u{fedb}', '\u{fedc}', '\u{feda}', '\u{06aa}', '\u{06aa}', '\u{06aa}',
    '\u{06aa}', '\u{06af}', '\u{fb94}', '\u{fb95}', '\u{fb93}', '\u{06af}', '\u{fb94}', '\u{fb95}',
    '\u{fb93}', '\u{06ab}', '\u{06ab}', '\u{06ab}', '\u{06ab}', '\u{06b1}', '\u{fb9c}', '\u{fb9d}',
    '\u{fb9b}', '\u{06b3}', '\u{fb98}', '\u{fb99}', '\u{fb97}', '\u{06b5}', '\u{06b5}', '\u{06b5}',
    '\u{06b5}', ' ', ' ', '\u{06ba}', ' ', ' ', '\u{fb9f}', '\u{06bc}', '\u{06bc}', '\u{06bc}',
    '\u{06bc}', '\u{06bb}', '\u{fba2}', '\u{fba3}', '\u{fba1}', '\u{06c6}', '\u{fbda}', ' ', ' ',
    '\u{06ca}', '\u{06ca}', '\u{06c1}', '\u{fba8}', '\u{fba9}', '\u{fba7}', '\u{06ce}', '\u{06ce}',
    '\u{06ce}', '\u{06ce}', '\u{06d2}', '\u{fbaf}', '\u{06d1}', '\u{06d1}', '\u{06d1}', '\u{06d1}',
    '\u{06c0}', '\u{fba5}',
];

const WP5_CS2: &[char] = &[
    '\u{0323}', '\u{0324}', '\u{02da}', '\u{0325}', '\u{02bc}', '\u{032d}', '\u{2017}', '_',
    '\u{0138}', '\u{032e}', '\u{033e}', '\u{2018}', ' ', '\u{02bd}', '\u{02db}', '\u{0327}',
    '\u{0321}', '\u{0322}', '\u{030d}', '\u{2019}', '\u{0329}', ' ', '\u{0621}', '\u{02be}',
    '\u{0306}', '\u{0310}', '\u{2032}', '\u{2034}',
];

const WP5_CS5: &[char] = &[
    '\u{2665}', '\u{2666}', '\u{2663}', '\u{2660}', '\u{2642}', '\u{2640}', '\u{263c}', '\u{263a}',
    '\u{263b}', '\u{266a}', '\u{266c}', '\u{25ac}', '\u{2302}', '\u{203c}', '\u{221a}', '\u{21a8}',
    '\u{2310}', '\u{2319}', '\u{25d8}', '\u{25d9}', '\u{21b5}', '\u{261e}', '\u{261c}', '\u{2713}',
    '\u{2610}', '\u{2612}', '\u{2639}', '\u{266f}', '\u{266d}', '\u{266e}', '\u{260e}', '\u{231a}',
    '\u{231b}', '\u{2104}', '\u{23b5}',
];

const WP5_CS8: &[char] = &[
    '\u{0391}', '\u{03b1}', '\u{0392}', '\u{03b2}', '\u{0392}', '\u{03d0}', '\u{0393}', '\u{03b3}',
    '\u{0394}', '\u{03b4}', '\u{0395}', '\u{03b5}', '\u{0396}', '\u{03b6}', '\u{0397}', '\u{03b7}',
    '\u{0398}', '\u{03b8}', '\u{0399}', '\u{03b9}', '\u{039a}', '\u{03ba}', '\u{039b}', '\u{03bb}',
    '\u{039c}', '\u{03bc}', '\u{039d}', '\u{03bd}', '\u{039e}', '\u{03be}', '\u{039f}', '\u{03bf}',
    '\u{03a0}', '\u{03c0}', '\u{03a1}', '\u{03c1}', '\u{03a3}', '\u{03c3}', '\u{03f9}', '\u{03db}',
    '\u{03a4}', '\u{03c4}', '\u{03a5}', '\u{03c5}', '\u{03a6}', '\u{03d5}', '\u{03a7}', '\u{03c7}',
    '\u{03a8}', '\u{03c8}', '\u{03a9}', '\u{03c9}', '\u{03ac}', '\u{03ad}', '\u{03ae}', '\u{03af}',
    '\u{03ca}', '\u{03cc}', '\u{03cd}', '\u{03cb}', '\u{03ce}', '\u{03b5}', '\u{03d1}', '\u{03f0}',
    '\u{03d6}', '\u{1fe5}', '\u{03d2}', '\u{03c6}', '\u{03c9}', '\u{037e}', '\u{0387}', '\u{0384}',
    '\u{00a8}', '\u{0385}', '\u{1fed}', '\u{1fef}', '\u{1fc0}', '\u{1fbd}', '\u{1fbf}', '\u{1fbe}',
    '\u{1fce}', '\u{1fde}', '\u{1fcd}', '\u{1fdd}', '\u{1fcf}', '\u{1fdf}', '\u{0384}', '\u{1fef}',
    '\u{1fc0}', '\u{1fbd}', '\u{1fbf}', '\u{1fce}', '\u{1fde}', '\u{1fcd}', '\u{1fdd}', '\u{1fcf}',
    '\u{1fdf}', '\u{1f70}', '\u{1fb6}', '\u{1fb3}', '\u{1fb4}', '\u{1fb7}', '\u{1f00}', '\u{1f04}',
    '\u{1f02}', '\u{1f06}', '\u{1f80}', '\u{1f84}', '\u{1f86}', '\u{1f01}', '\u{1f05}', '\u{1f03}',
    '\u{1f07}', '\u{1f81}', '\u{1f85}', '\u{1f87}', '\u{1f72}', '\u{1f10}', '\u{1f14}', '\u{1f13}',
    '\u{1f11}', '\u{1f15}', '\u{1f13}', '\u{1f74}', '\u{1fc6}', '\u{1fc3}', '\u{1fc4}', '\u{1fc2}',
    '\u{1fc7}', '\u{1f20}', '\u{1f24}', '\u{1f22}', '\u{1f26}', '\u{1f90}', '\u{1f94}', '\u{1f96}',
    '\u{1f21}', '\u{1f25}', '\u{1f23}', '\u{1f27}', '\u{1f91}', '\u{1f95}', '\u{1f97}', '\u{1f76}',
    '\u{1fd6}', '\u{0390}', '\u{1fd2}', '\u{1f30}', '\u{1f34}', '\u{1f32}', '\u{1f36}', '\u{1f31}',
    '\u{1f35}', '\u{1f33}', '\u{1f37}', '\u{1f78}', '\u{1f40}', '\u{1f44}', '\u{1f42}', '\u{1f41}',
    '\u{1f45}', '\u{1f43}', '\u{1f7a}', '\u{1fe6}', '\u{03b0}', '\u{1fe3}', '\u{1f50}', '\u{1f54}',
    '\u{1f52}', '\u{1f56}', '\u{1f51}', '\u{1f55}', '\u{1f53}', '\u{1f57}', '\u{1f7c}', '\u{1ff6}',
    '\u{1ff3}', '\u{1ff4}', '\u{1ff2}', '\u{1ff7}', '\u{1f60}', '\u{1f64}', '\u{1f62}', '\u{1f66}',
    '\u{1fa0}', '\u{1fa4}', '\u{1fa6}', '\u{1f61}', '\u{1f65}', '\u{1f63}', '\u{1f67}', '\u{1fa1}',
    '\u{1fa5}', '\u{1fa7}', '\u{0374}', '\u{0375}', '\u{03db}', '\u{03dd}', '\u{03d9}', '\u{03e1}',
    '\u{0386}', '\u{0388}', '\u{0389}', '\u{038a}', '\u{038c}', '\u{038e}', '\u{038f}', '\u{03aa}',
    '\u{03ab}', '\u{1fe5}',
];

const WP5_CS9: &[char] = &[
    '\u{05d0}', '\u{05d1}', '\u{05d2}', '\u{05d3}', '\u{05d4}', '\u{05d5}', '\u{05d6}', '\u{05d7}',
    '\u{05d8}', '\u{05d9}', '\u{05da}', '\u{05db}', '\u{05dc}', '\u{05dd}', '\u{05de}', '\u{05df}',
    '\u{05e0}', '\u{05e1}', '\u{05e2}', '\u{05e3}', '\u{05e4}', '\u{05e5}', '\u{05e6}', '\u{05e7}',
    '\u{05e8}', '\u{05e9}', '\u{05ea}', '\u{05be}', '\u{05c0}', '\u{05c3}', '\u{05f3}', '\u{05f4}',
    '\u{05b0}', '\u{05b1}', '\u{05b2}', '\u{05b3}', '\u{05b4}', '\u{05b5}', '\u{05b6}', '\u{05b7}',
    '\u{05b8}', '\u{05b9}', '\u{05ba}', '\u{05bb}', '\u{05bc}', '\u{05bd}', '\u{05bf}', '\u{05b7}',
    '\u{fbe1}', '\u{05f0}', '\u{05f1}', '\u{05f2}', '\u{0591}', '\u{0596}', '\u{05ad}', '\u{05a4}',
    '\u{059a}', '\u{059b}', '\u{05a3}', '\u{05a5}', '\u{05a6}', '\u{05a7}', '\u{09aa}', '\u{0592}',
    '\u{0593}', '\u{0594}', '\u{0595}', '\u{0597}', '\u{0598}', '\u{0599}', '\u{05a8}', '\u{059c}',
    '\u{059d}', '\u{059e}', '\u{05a1}', '\u{05a9}', '\u{05a0}', '\u{059f}', '\u{05ab}', '\u{05ac}',
    '\u{05af}', '\u{05c4}', '\u{0544}', '\u{05d0}', '\u{fb31}', '\u{fb32}', '\u{fb33}', '\u{fb34}',
    '\u{fb35}', '\u{fb4b}', '\u{fb36}', '\u{05d7}', '\u{fb38}', '\u{fb39}', '\u{fb3b}', '\u{fb3a}',
    '\u{05da}', '\u{05da}', '\u{05da}', '\u{05da}', '\u{05da}', '\u{05da}', '\u{fb3c}', '\u{fb3e}',
    '\u{fb40}', '\u{05df}', '\u{fb41}', '\u{fb44}', '\u{fb46}', '\u{fb47}', '\u{fb2b}', '\u{fb2d}',
    '\u{fb2a}', '\u{fb2c}', '\u{fb4a}', '\u{fb4c}', '\u{fb4e}', '\u{fb1f}', '\u{fb1d}',
];

const WP5_CS10: &[char] = &[
    '\u{0410}', '\u{0430}', '\u{0411}', '\u{0431}', '\u{0412}', '\u{0432}', '\u{0413}', '\u{0433}',
    '\u{0414}', '\u{0434}', '\u{0415}', '\u{0435}', '\u{0401}', '\u{0451}', '\u{0416}', '\u{0436}',
    '\u{0417}', '\u{0437}', '\u{0418}', '\u{0438}', '\u{0419}', '\u{0439}', '\u{041a}', '\u{043a}',
    '\u{041b}', '\u{043b}', '\u{041c}', '\u{043c}', '\u{041d}', '\u{043d}', '\u{041e}', '\u{043e}',
    '\u{041f}', '\u{043f}', '\u{0420}', '\u{0440}', '\u{0421}', '\u{0441}', '\u{0422}', '\u{0442}',
    '\u{0423}', '\u{0443}', '\u{0424}', '\u{0444}', '\u{0425}', '\u{0445}', '\u{0426}', '\u{0446}',
    '\u{0427}', '\u{0447}', '\u{0428}', '\u{0448}', '\u{0429}', '\u{0449}', '\u{042a}', '\u{044a}',
    '\u{042b}', '\u{044b}', '\u{042c}', '\u{044c}', '\u{042d}', '\u{044d}', '\u{042e}', '\u{044e}',
    '\u{042f}', '\u{044f}', '\u{0490}', '\u{0491}', '\u{0402}', '\u{0452}', '\u{0403}', '\u{0453}',
    '\u{0404}', '\u{0454}', '\u{0405}', '\u{0455}', '\u{0406}', '\u{0456}', '\u{0407}', '\u{0457}',
    '\u{0408}', '\u{0458}', '\u{0409}', '\u{0459}', '\u{040a}', '\u{045a}', '\u{040b}', '\u{045b}',
    '\u{040c}', '\u{045c}', '\u{040e}', '\u{045e}', '\u{040f}', '\u{045f}', '\u{0462}', '\u{0463}',
    '\u{0472}', '\u{0473}', '\u{0474}', '\u{0475}', '\u{046a}', '\u{046b}', '\u{a640}', '\u{a641}',
    '\u{0429}', '\u{0449}', '\u{04c0}', '\u{04cf}', '\u{0466}', '\u{0467}', '\u{0000}', '\u{0000}',
    '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}',
    '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}',
    '\u{0000}', '\u{0000}', '\u{0400}', '\u{0450}', '\u{0000}', '\u{0000}', '\u{040d}', '\u{045d}',
    '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}',
    '\u{0000}', '\u{0000}', '\u{0000}', '\u{0000}', '\u{0301}', '\u{0300}',
];

const WP5_CS11: &[char] = &[
    '\u{3041}', '\u{3043}', '\u{3045}', '\u{3047}', '\u{3049}', '\u{3053}', '\u{3083}', '\u{3085}',
    '\u{3087}', '\u{3094}', '\u{3095}', '\u{3096}', '\u{3042}', '\u{3044}', '\u{3046}', '\u{3048}',
    '\u{304a}', '\u{304b}', '\u{304d}', '\u{3047}', '\u{3051}', '\u{3053}', '\u{304c}', '\u{304e}',
    '\u{3050}', '\u{3052}', '\u{3054}', '\u{3055}', '\u{3057}', '\u{3059}', '\u{305b}', '\u{305d}',
    '\u{3056}', '\u{3058}', '\u{305a}', '\u{305c}', '\u{305e}', '\u{305f}', '\u{3051}', '\u{3064}',
    '\u{3066}', '\u{3068}', '\u{3060}', '\u{3062}', '\u{3065}', '\u{3067}', '\u{3069}', '\u{306a}',
    '\u{306b}', '\u{306c}', '\u{306d}', '\u{306e}', '\u{306f}', '\u{3072}', '\u{3075}', '\u{3078}',
    '\u{307b}', '\u{3070}', '\u{3073}', '\u{3076}', '\u{3079}', '\u{307c}', '\u{3071}', '\u{3074}',
    '\u{3077}', '\u{307a}', '\u{307d}', '\u{307e}', '\u{307f}', '\u{3080}', '\u{3081}', '\u{3082}',
    '\u{3084}', '\u{3086}', '\u{3088}', '\u{3089}', '\u{308a}', '\u{308b}', '\u{308c}', '\u{308d}',
    '\u{308e}', '\u{3092}', '\u{3093}', '\u{3014}', '\u{3015}', '\u{ff3b}', '\u{ff3d}', '\u{300c}',
    '\u{300d}', '\u{300c}', '\u{300d}', '\u{302a}', '\u{3002}', '\u{3001}', '\u{309d}', '\u{309e}',
    '\u{3003}', '\u{30fc}', '\u{309b}', '\u{309c}', '\u{30a1}', '\u{30a3}', '\u{30a5}', '\u{30a7}',
    '\u{30a9}', '\u{30c3}', '\u{30e3}', '\u{30e5}', '\u{3057}', '\u{30f4}', '\u{30f5}', '\u{30f6}',
    '\u{30a2}', '\u{30a4}', '\u{30a6}', '\u{30a8}', '\u{30aa}', '\u{30ab}', '\u{30ad}', '\u{30af}',
    '\u{30b1}', '\u{30b3}', '\u{30ac}', '\u{30ae}', '\u{30b0}', '\u{30b2}', '\u{30b4}', '\u{30b5}',
    '\u{30c4}', '\u{30b9}', '\u{30bb}', '\u{30bd}', '\u{30b6}', '\u{30b8}', '\u{30ba}', '\u{30bc}',
    '\u{30be}', '\u{30bf}', '\u{30c1}', '\u{30c4}', '\u{30c6}', '\u{30c8}', '\u{30c0}', '\u{30c2}',
    '\u{30c5}', '\u{30c7}', '\u{30c9}', '\u{30ca}', '\u{30cb}', '\u{30cc}', '\u{30cd}', '\u{30ce}',
    '\u{30cf}', '\u{30d2}', '\u{30d5}', '\u{30d8}', '\u{03d0}', '\u{30db}', '\u{30d3}', '\u{30d6}',
    '\u{30d9}', '\u{30dc}', '\u{30d1}', '\u{30d4}', '\u{30d7}', '\u{30da}', '\u{30dd}', '\u{30de}',
    '\u{30df}', '\u{30e0}', '\u{30e1}', '\u{30e2}', '\u{30e4}', '\u{30e6}', '\u{30e8}', '\u{30e9}',
    '\u{30ea}', '\u{30ab}', '\u{30ec}', '\u{30ed}', '\u{30ef}', '\u{30f2}', '\u{30f3}', '\u{30fd}',
    '\u{30fe}',
];

const WP5_CS12: &[char] = &[
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
    ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ',
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::InputFormat;

    fn convert(bytes: Vec<u8>) -> Result<DoclingDocument, ConversionError> {
        WpdBackend.convert(&SourceDocument::from_bytes(
            "t.wpd",
            InputFormat::WordPerfect,
            bytes,
        ))
    }

    /// A minimal WP6 file: 16-byte prefix pointing straight at `body`.
    fn wp6(body: &[u8]) -> Vec<u8> {
        let mut v = MAGIC.to_vec();
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&[PRODUCT_WORDPERFECT, FILE_TYPE_DOCUMENT, 2, 1, 0, 0, 0, 0]);
        v.extend_from_slice(body);
        v
    }

    fn wp5(body: &[u8]) -> Vec<u8> {
        let mut v = wp6(body);
        v[10] = 0;
        v
    }

    fn md(doc: &DoclingDocument) -> String {
        doc.export_to_markdown()
    }

    #[test]
    fn wp6_paragraphs_soft_breaks_and_deleted_text() {
        // "Hello" soft-wrap "world" hard return; deleted "gone"; "Next".
        let body = [
            b"Hello".as_slice(),
            &[0xD0, 0x01, 0x05, 0x00, 0xD0], // EOL group, soft end of column
            b"world",
            &[0xCC],                         // hard return
            &[0xF1, 0x00, 0x00, 0x00, 0xF1], // undo start
            b"gone",
            &[0xF1, 0x01, 0x00, 0x00, 0xF1], // undo end
            b"Next",
        ]
        .concat();
        let doc = convert(wp6(&body)).unwrap();
        assert_eq!(md(&doc), "Hello world\n\nNext\n");
    }

    #[test]
    fn wp6_attributes_and_extended_chars() {
        // bold on(12) "Bold" bold off, space, italic on(8) "it" off, ext char
        // charset 4 #28 (typographic closing single quote) and intl char 1 (å).
        let body = [
            &[0xF2, 12, 0xF2][..],
            b"Bold",
            &[0xF3, 12, 0xF3],
            &[0x80],
            &[0xF2, 8, 0xF2],
            b"it",
            &[0xF3, 8, 0xF3],
            &[0x80, 0xF0, 28, 4, 0xF0, 0x01],
        ]
        .concat();
        let doc = convert(wp6(&body)).unwrap();
        assert_eq!(md(&doc), "**Bold** *it* \u{2019}\u{e5}\n");
        let Node::InlineGroup { runs, .. } = &doc.nodes[0] else {
            panic!("expected inline runs, got {:?}", doc.nodes[0]);
        };
        // whitespace-only runs are dropped from the structured runs
        assert_eq!(runs.len(), 3, "{runs:?}");
        assert!(runs[0].bold && !runs[0].italic);
        assert!(runs[1].italic && !runs[1].bold);
        assert!(runs[2].is_plain());
    }

    #[test]
    fn wp6_table_from_eol_group_cells() {
        let cell = |t: &[u8]| [t, &[0xD0, 10, 0x05, 0x00, 0xD0]].concat();
        let row_end = [0xD0u8, 11, 0x05, 0x00, 0xD0];
        let table_off = [0xD0u8, 15, 0x05, 0x00, 0xD0];
        let body = [
            b"Before".as_slice(),
            &[0xCC],
            &cell(b"a"),
            b"b",
            &row_end,
            &cell(b"c"),
            b"d",
            &row_end,
            &table_off,
            b"After",
        ]
        .concat();
        let doc = convert(wp6(&body)).unwrap();
        let Node::Table(t) = &doc.nodes[1] else {
            panic!("expected a table, got {:?}", doc.nodes[1]);
        };
        assert_eq!(t.rows, vec![vec!["a", "b"], vec!["c", "d"]]);
        assert!(matches!(&doc.nodes[0], Node::Paragraph { text } if text == "Before"));
        assert!(matches!(&doc.nodes[2], Node::Paragraph { text } if text == "After"));
    }

    #[test]
    fn wp5_codes() {
        // bold pair, hard hyphen, apostrophe (charset 1 #9), deletable soft
        // return, hard return, skipped variable-length group.
        let body = [
            &[0xC3, 12, 0xC3][..],
            b"TITLE:",
            &[0xC4, 12, 0xC4],
            b" long",
            &[0xA9],
            b"term ",
            &[0x90],
            b"it",
            &[0xC0, 9, 1, 0xC0],
            b"s",
            &[0x0A],
            &[0xD9, 5, 0x03, 0x00, 0xAA, 0xBB, 0xCC], // group: 3 payload bytes
            b"End",
        ]
        .concat();
        let doc = convert(wp5(&body)).unwrap();
        assert_eq!(md(&doc), "**TITLE:** long-term it\u{2019}s\n\nEnd\n");
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        let e = convert(b"not a wp file at all".to_vec())
            .unwrap_err()
            .to_string();
        assert!(e.contains("not a WordPerfect document"), "{e}");
        let mut enc = wp6(b"x");
        enc[12] = 1;
        let e = convert(enc).unwrap_err().to_string();
        assert!(e.contains("password-protected"), "{e}");
        let mut mac = wp6(b"x");
        mac[9] = 44;
        let e = convert(mac).unwrap_err().to_string();
        assert!(e.contains("Macintosh"), "{e}");
        let mut v7 = wp6(b"x");
        v7[10] = 7;
        let e = convert(v7).unwrap_err().to_string();
        assert!(e.contains("unsupported WordPerfect version 7.1"), "{e}");
    }

    #[test]
    fn fixture_rejects_are_targeted() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/wordperfect/invalid");
        let fuzz = std::fs::read(dir.join("fuzzed_prefix.wpd")).unwrap();
        assert!(convert(fuzz)
            .unwrap_err()
            .to_string()
            .contains("not a WordPerfect"));
        let mac = std::fs::read(dir.join("wp_mac3.wpd")).unwrap();
        assert!(convert(mac).unwrap_err().to_string().contains("Macintosh"));
    }
}
