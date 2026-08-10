//! RTF backend (issue #209) — a docling.rs extension; Python docling has no
//! RTF backend (it converts RTF only by shelling out to LibreOffice), so there
//! is no byte-conformance target: output follows the DOCX backend's shapes.
//!
//! RTF is a plain-text control-word format (`\b`, `\par`, `\trowd`, …) with
//! `{}` groups scoping formatting state, so this is a hand-rolled tokenizer in
//! the spirit of the AsciiDoc/LaTeX backends — no external parser crate, pure
//! Rust, wasm-clean. What it covers:
//!
//! - paragraphs, `**bold**` / `*italic*` / `~~strike~~` runs (baked into the
//!   text, the docling.rs convention), `\line` breaks, `\page` page breaks
//! - headings from `\outlinelevel` or the stylesheet (`\sN` whose stylesheet
//!   name is "heading N")
//! - lists via the `\listtext`/`\pntext` compatibility markers (`·`-style
//!   markers → bullets, `1.`-style → ordered items with their number)
//! - tables: `\trowd` … `\cell` … `\row` rows, ragged rows padded; nested
//!   groups inside cells contribute their text
//! - embedded pictures: `\pict` with `\pngblip`/`\jpegblip` hex data becomes a
//!   [`Node::Picture`] with the decoded bytes
//! - encodings: `\'xx` bytes through the `\ansicpg` codepage (1252 default,
//!   1250/1251 supported), `\uN` unicode with the `\uc` skip protocol
//!
//! Header/footer/info/font/color destinations are skipped, as are unknown
//! `{\*\…}` destinations (per spec, a reader that does not understand a
//! starred destination must ignore it). Fields keep their `\fldrslt` (the
//! last rendered result) and drop the instruction.

use docling_core::{DoclingDocument, Node, PictureImage, Table};

use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;

pub struct RtfBackend;

impl DeclarativeBackend for RtfBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        // RTF is 7-bit ASCII by design (non-ASCII travels as \'xx / \uN), but
        // real files are sometimes saved with raw high bytes — treat those
        // through the document codepage rather than failing UTF-8 validation.
        let text: String = source.bytes.iter().map(|&b| b as char).collect();
        if !text.trim_start().starts_with("{\\rtf") {
            return Err(ConversionError::Parse("rtf: missing {\\rtf header".into()));
        }
        let mut doc = DoclingDocument::new(&source.name);
        Parser::new(&text).run(&mut doc);
        Ok(doc)
    }
}

/// Per-group state, cloned on `{` and restored on `}` (RTF's scoping rule).
#[derive(Clone, Default)]
struct GroupState {
    bold: bool,
    italic: bool,
    strike: bool,
    /// Inside a destination whose content must not become body text
    /// (fonttbl, info, header, an unknown `{\*\…}`, …).
    skip: bool,
    /// `\ucN`: how many fallback characters follow each `\uN`.
    uc: usize,
    /// `\intbl`: this paragraph belongs to the current table row.
    in_table: bool,
    /// `\sN` style handle (heading lookup via the stylesheet).
    style: Option<i32>,
    /// `\outlinelevelN` (0-based).
    outline: Option<u8>,
    /// `\ilvlN` list nesting level (0-based).
    ilvl: u8,
}

/// One formatted run of paragraph text.
struct Run {
    text: String,
    bold: bool,
    italic: bool,
    strike: bool,
}

/// The pending `\listtext`/`\pntext` marker for the paragraph being built:
/// `(ordered, display number, raw marker)`.
type ListMarker = (bool, u64, String);

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    stack: Vec<GroupState>,
    state: GroupState,
    codepage: u32,
    /// Style handle → heading level, from `{\stylesheet …}` names.
    heading_styles: Vec<(i32, u8)>,
    runs: Vec<Run>,
    list_marker: Option<ListMarker>,
    prev_was_list: bool,
    /// The table being assembled: completed rows + the current row's cells.
    rows: Vec<Vec<String>>,
    cells: Vec<String>,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        Parser {
            bytes: text.as_bytes(),
            pos: 0,
            stack: Vec::new(),
            state: GroupState {
                uc: 1,
                ..GroupState::default()
            },
            codepage: 1252,
            heading_styles: Vec::new(),
            runs: Vec::new(),
            list_marker: None,
            prev_was_list: false,
            rows: Vec::new(),
            cells: Vec::new(),
        }
    }

    fn run(&mut self, doc: &mut DoclingDocument) {
        while let Some(b) = self.next_byte() {
            match b {
                b'{' => self.stack.push(self.state.clone()),
                b'}' => {
                    if let Some(prev) = self.stack.pop() {
                        self.state = prev;
                    }
                }
                b'\\' => self.control(doc),
                b'\r' | b'\n' => {} // literal newlines are insignificant
                _ => self.text_byte(b),
            }
        }
        self.flush_paragraph(doc);
        self.flush_table(doc);
    }

    fn next_byte(&mut self) -> Option<u8> {
        let b = self.bytes.get(self.pos).copied();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// A control word (`\word` + optional signed integer + one optional
    /// delimiting space) or control symbol (single non-alphabetic byte).
    fn control(&mut self, doc: &mut DoclingDocument) {
        let Some(b) = self.peek() else { return };
        if !b.is_ascii_alphabetic() {
            self.pos += 1;
            self.control_symbol(b);
            return;
        }
        let start = self.pos;
        while self.peek().is_some_and(|b| b.is_ascii_alphabetic()) {
            self.pos += 1;
        }
        let word: String = self.bytes[start..self.pos]
            .iter()
            .map(|&b| b as char)
            .collect();
        let mut param: Option<i64> = None;
        let num_start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.pos > num_start {
            param = String::from_utf8_lossy(&self.bytes[num_start..self.pos])
                .parse()
                .ok();
        }
        // The single space after a control word is part of the control word.
        if self.peek() == Some(b' ') {
            self.pos += 1;
        }
        self.control_word(&word, param, doc);
    }

    fn control_symbol(&mut self, b: u8) {
        match b {
            b'\'' => {
                // \'xx — one byte in the document codepage.
                let hex: String = (0..2)
                    .filter_map(|_| self.next_byte().map(|b| b as char))
                    .collect();
                if let Ok(v) = u8::from_str_radix(&hex, 16) {
                    let ch = decode_byte(v, self.codepage);
                    self.push_char(ch);
                }
            }
            b'*' => {
                // {\*\dest …}: ignorable destination. The known ones we *do*
                // read re-enable themselves in control_word.
                self.state.skip = true;
            }
            b'~' => self.push_char('\u{00A0}'),
            b'-' => {} // optional hyphen
            b'_' => self.push_char('-'),
            b'\\' | b'{' | b'}' => self.push_char(b as char),
            b'\r' | b'\n' => {} // \<newline> == \par in old writers; treat as space
            _ => {}
        }
    }

    fn control_word(&mut self, word: &str, param: Option<i64>, doc: &mut DoclingDocument) {
        match word {
            "ansicpg" => {
                if let Some(cp) = param {
                    self.codepage = cp as u32;
                }
            }
            "uc" => self.state.uc = param.unwrap_or(1).max(0) as usize,
            "u" => {
                if let Some(v) = param {
                    // Signed 16-bit: negative values wrap (e.g. -3999 → 61537).
                    let cp = if v < 0 { v + 65536 } else { v } as u32;
                    if let Some(ch) = char::from_u32(cp) {
                        self.push_char(ch);
                    }
                    self.skip_unicode_fallback();
                }
            }
            // Formatting toggles: \b on, \b0 off.
            "b" => self.state.bold = param != Some(0),
            "i" => self.state.italic = param != Some(0),
            "strike" => self.state.strike = param != Some(0),
            "plain" => {
                self.state.bold = false;
                self.state.italic = false;
                self.state.strike = false;
            }
            "s" => self.state.style = param.map(|p| p as i32),
            "outlinelevel" => self.state.outline = param.map(|p| p.clamp(0, 8) as u8),
            "ilvl" => self.state.ilvl = param.unwrap_or(0).clamp(0, 8) as u8,
            "pard" => {
                // Paragraph-default reset clears paragraph-scoped properties.
                self.state.style = None;
                self.state.outline = None;
                self.state.in_table = false;
                self.state.ilvl = 0;
            }
            "intbl" => self.state.in_table = true,
            "par" => self.flush_paragraph(doc),
            "line" => self.push_char('\n'),
            "tab" => self.push_char('\t'),
            "page" => {
                self.flush_paragraph(doc);
                self.flush_table(doc);
                doc.push(Node::PageBreak);
            }
            "cell" => self.end_cell(),
            "row" => self.end_row(),
            "nestcell" | "nestrow" => {} // nested tables flatten into the cell
            "stylesheet" => self.read_stylesheet(),
            "pict" => self.read_picture(doc),
            "fonttbl" | "colortbl" | "info" | "listtable" | "listoverridetable" | "header"
            | "headerl" | "headerr" | "headerf" | "footer" | "footerl" | "footerr" | "footerf"
            | "footnote" | "ftnsep" | "ftnsepc" => {
                self.state.skip = true;
            }
            // A field's instruction is machinery; its \fldrslt is the rendered
            // text and reads as ordinary content.
            "fldinst" => self.state.skip = true,
            "fldrslt" => self.state.skip = false,
            // The list-marker compatibility text: capture it to type the
            // paragraph, but keep it out of the body text.
            "listtext" | "pntext" => {
                let marker = self.capture_group_text();
                self.list_marker = Some(classify_marker(&marker));
            }
            _ => {} // unknown control words are ignored per spec
        }
    }

    /// After `\uN`, skip the next `uc` fallback characters (each `\'xx`
    /// counts as one).
    fn skip_unicode_fallback(&mut self) {
        for _ in 0..self.state.uc {
            match self.peek() {
                Some(b'\\') if self.bytes.get(self.pos + 1) == Some(&b'\'') => {
                    self.pos += 4; // \'xx
                }
                Some(b'{') | Some(b'}') | Some(b'\\') | None => break,
                _ => self.pos += 1,
            }
        }
    }

    /// Consume the rest of the current group (after `\listtext`-style words)
    /// and return its plain text; formatting and destinations inside are
    /// dropped. Assumes the group's `{` was already consumed.
    fn capture_group_text(&mut self) -> String {
        let mut depth = 0usize;
        let mut out = String::new();
        while let Some(b) = self.next_byte() {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    if depth == 0 {
                        // Re-run the group close on the main loop's stack.
                        self.pos -= 1;
                        break;
                    }
                    depth -= 1;
                }
                b'\\' => {
                    // Only \'xx and \tab contribute text inside a marker.
                    if self.peek() == Some(b'\'') {
                        self.pos += 1;
                        let hex: String = (0..2)
                            .filter_map(|_| self.next_byte().map(|b| b as char))
                            .collect();
                        if let Ok(v) = u8::from_str_radix(&hex, 16) {
                            out.push(decode_byte(v, self.codepage));
                        }
                    } else {
                        while self
                            .peek()
                            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'-')
                        {
                            self.pos += 1;
                        }
                        if self.peek() == Some(b' ') {
                            self.pos += 1;
                        }
                    }
                }
                b'\r' | b'\n' => {}
                _ => out.push(b as char),
            }
        }
        out
    }

    /// `{\stylesheet {\s1 …heading 1;}{\s2 …heading 2;}…}`: map style handles
    /// whose *name* (the trailing plain text before `;`) is "heading N".
    fn read_stylesheet(&mut self) {
        let mut depth = 0usize;
        let mut style: Option<i32> = None;
        let mut name = String::new();
        while let Some(b) = self.next_byte() {
            match b {
                b'{' => {
                    depth += 1;
                    style = None;
                    name.clear();
                }
                b'}' => {
                    if depth == 0 {
                        self.pos -= 1; // main loop pops the stylesheet group
                        break;
                    }
                    depth -= 1;
                }
                b';' => {
                    if let (Some(s), Some(level)) = (style, heading_level(&name)) {
                        self.heading_styles.push((s, level));
                    }
                }
                b'\\' => {
                    let start = self.pos;
                    while self.peek().is_some_and(|b| b.is_ascii_alphabetic()) {
                        self.pos += 1;
                    }
                    let word: String = self.bytes[start..self.pos]
                        .iter()
                        .map(|&b| b as char)
                        .collect();
                    let num_start = self.pos;
                    if self.peek() == Some(b'-') {
                        self.pos += 1;
                    }
                    while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                        self.pos += 1;
                    }
                    if word == "s" && self.pos > num_start {
                        style = String::from_utf8_lossy(&self.bytes[num_start..self.pos])
                            .parse()
                            .ok();
                    }
                    if self.peek() == Some(b' ') {
                        self.pos += 1;
                    }
                }
                b'\r' | b'\n' => {}
                _ => name.push(b as char),
            }
        }
    }

    /// `{\pict \pngblip|\jpegblip … <hex>}` → a picture node with the decoded
    /// bytes. Metafile-only pictures (`\wmetafile`/`\emfblip`) are skipped —
    /// no portable decoder — as is the rare inline-binary `\bin` form.
    fn read_picture(&mut self, doc: &mut DoclingDocument) {
        let mut depth = 0usize;
        let mut mimetype: Option<&'static str> = None;
        let mut hex = String::new();
        let mut skip_binary = false;
        while let Some(b) = self.next_byte() {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    if depth == 0 {
                        self.pos -= 1;
                        break;
                    }
                    depth -= 1;
                }
                b'\\' => {
                    let start = self.pos;
                    while self.peek().is_some_and(|b| b.is_ascii_alphabetic()) {
                        self.pos += 1;
                    }
                    let word: String = self.bytes[start..self.pos]
                        .iter()
                        .map(|&b| b as char)
                        .collect();
                    while self.peek().is_some_and(|b| b.is_ascii_digit() || b == b'-') {
                        self.pos += 1;
                    }
                    if self.peek() == Some(b' ') {
                        self.pos += 1;
                    }
                    match word.as_str() {
                        "pngblip" => mimetype = Some("image/png"),
                        "jpegblip" => mimetype = Some("image/jpeg"),
                        "bin" => skip_binary = true,
                        _ => {}
                    }
                }
                b if b.is_ascii_hexdigit() => hex.push(b as char),
                _ => {}
            }
        }
        let Some(mimetype) = mimetype else { return };
        if skip_binary || hex.is_empty() {
            return;
        }
        let data: Vec<u8> = hex
            .as_bytes()
            .chunks_exact(2)
            .filter_map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
            .collect();
        let (width, height) = image_size(mimetype, &data).unwrap_or((0, 0));
        doc.push(Node::Picture {
            caption: None,
            image: Some(PictureImage {
                mimetype: mimetype.to_string(),
                width,
                height,
                data,
            }),
            classification: None,
        });
    }

    fn push_char(&mut self, ch: char) {
        if self.state.skip {
            return;
        }
        let (bold, italic, strike) = (self.state.bold, self.state.italic, self.state.strike);
        match self.runs.last_mut() {
            Some(run) if run.bold == bold && run.italic == italic && run.strike == strike => {
                run.text.push(ch);
            }
            _ => self.runs.push(Run {
                text: ch.to_string(),
                bold,
                italic,
                strike,
            }),
        }
    }

    fn text_byte(&mut self, b: u8) {
        self.push_char(decode_byte(b, self.codepage));
    }

    /// Render the accumulated runs as one Markdown-baked string (the docling.rs
    /// convention — `**bold**`, `*italic*`, `~~strike~~`), keeping run-edge
    /// whitespace outside the markers so the Markdown stays valid.
    fn take_text(&mut self) -> String {
        let mut out = String::new();
        for run in self.runs.drain(..) {
            let trimmed = run.text.trim();
            if trimmed.is_empty() {
                out.push_str(&run.text);
                continue;
            }
            let lead = &run.text[..run.text.len() - run.text.trim_start().len()];
            let trail = &run.text[run.text.trim_end().len()..];
            let mut s = trimmed.to_string();
            if run.bold {
                s = format!("**{s}**");
            }
            if run.italic {
                s = format!("*{s}*");
            }
            if run.strike {
                s = format!("~~{s}~~");
            }
            out.push_str(lead);
            out.push_str(&s);
            out.push_str(trail);
        }
        out.trim().to_string()
    }

    fn end_cell(&mut self) {
        let text = self.take_text();
        self.cells.push(text);
        self.list_marker = None;
    }

    fn end_row(&mut self) {
        if !self.cells.is_empty() {
            self.rows.push(std::mem::take(&mut self.cells));
        }
    }

    fn flush_table(&mut self, doc: &mut DoclingDocument) {
        self.end_row();
        if self.rows.is_empty() {
            return;
        }
        let mut rows = std::mem::take(&mut self.rows);
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        for row in &mut rows {
            row.resize(width, String::new());
        }
        doc.push(Node::Table(Table {
            rows,
            location: None,
            structure: None,
            cell_blocks: None,
            caption: None,
        }));
        self.prev_was_list = false;
    }

    fn flush_paragraph(&mut self, doc: &mut DoclingDocument) {
        if self.state.in_table {
            // \par inside a cell is a soft break within that cell's text.
            self.push_char('\n');
            return;
        }
        // Leaving the table region: emit the assembled table first.
        self.flush_table(doc);

        let marker = self.list_marker.take();
        let text = self.take_text();
        if text.is_empty() {
            self.prev_was_list = false;
            return;
        }
        let heading = self
            .state
            .outline
            .map(|o| o + 1)
            .or_else(|| {
                let style = self.state.style?;
                self.heading_styles
                    .iter()
                    .find(|(s, _)| *s == style)
                    .map(|(_, level)| *level)
            })
            .map(|level| level.clamp(1, 6));
        if let Some(level) = heading {
            doc.push(Node::Heading { level, text });
            self.prev_was_list = false;
            return;
        }
        if let Some((ordered, number, marker)) = marker {
            let first_in_list = !self.prev_was_list;
            let level = self.state.ilvl;
            if ordered {
                doc.push(Node::ListItem {
                    ordered,
                    number,
                    first_in_list,
                    text,
                    level,
                    marker: Some(marker),
                    location: None,
                    dclx: None,
                    href: None,
                    layer: None,
                });
            } else {
                doc.push(Node::ListItem {
                    ordered: false,
                    number: 0,
                    first_in_list,
                    text,
                    level,
                    marker: None,
                    location: None,
                    dclx: None,
                    href: None,
                    layer: None,
                });
            }
            self.prev_was_list = true;
            return;
        }
        doc.push(Node::Paragraph { text });
        self.prev_was_list = false;
    }
}

/// "heading 1" … "heading 9" (case-insensitive) → level.
fn heading_level(style_name: &str) -> Option<u8> {
    let name = style_name.trim().to_ascii_lowercase();
    let rest = name.strip_prefix("heading ")?;
    rest.parse::<u8>().ok().filter(|n| (1..=9).contains(n))
}

/// Type a `\listtext` marker: `1.` / `12)` → ordered with that number,
/// anything else (`·`, `-`, `o`, Symbol-font bullets) → bullet.
fn classify_marker(marker: &str) -> (bool, u64, String) {
    let m = marker.trim().trim_end_matches(['.', ')']);
    if let Ok(n) = m.parse::<u64>() {
        (true, n, format!("{n}."))
    } else {
        (false, 0, String::new())
    }
}

/// One byte → char through the document codepage. ASCII is universal; the
/// supported high-byte pages are Windows-1252 (default), -1250 and -1251.
fn decode_byte(b: u8, codepage: u32) -> char {
    if b < 0x80 {
        return b as char;
    }
    let table: &[u16; 128] = match codepage {
        1250 => &CP1250,
        1251 => &CP1251,
        _ => &CP1252,
    };
    char::from_u32(table[(b - 0x80) as usize] as u32).unwrap_or('\u{FFFD}')
}

/// PNG IHDR / JPEG SOF dimensions, best-effort (`None` → 0×0 metadata).
fn image_size(mimetype: &str, data: &[u8]) -> Option<(u32, u32)> {
    match mimetype {
        "image/png" => {
            if data.len() < 24 || &data[..8] != b"\x89PNG\r\n\x1a\n" {
                return None;
            }
            let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
            Some((be(&data[16..20]), be(&data[20..24])))
        }
        "image/jpeg" => {
            // Walk the segment chain to the first SOFn frame header.
            let mut i = 2usize;
            while i + 9 < data.len() {
                if data[i] != 0xFF {
                    return None;
                }
                let marker = data[i + 1];
                let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
                if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                    let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                    let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                    return Some((w, h));
                }
                i += 2 + len;
            }
            None
        }
        _ => None,
    }
}

/// Windows-1252, upper half (0x80–0xFF).
const CP1252: [u16; 128] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178, 0x00A0, 0x00A1, 0x00A2, 0x00A3,
    0x00A4, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x00B9, 0x00BA, 0x00BB,
    0x00BC, 0x00BD, 0x00BE, 0x00BF, 0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF, 0x00D0, 0x00D1, 0x00D2, 0x00D3,
    0x00D4, 0x00D5, 0x00D6, 0x00D7, 0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x00DD, 0x00DE, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7, 0x00E8, 0x00E9, 0x00EA, 0x00EB,
    0x00EC, 0x00ED, 0x00EE, 0x00EF, 0x00F0, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x00FD, 0x00FE, 0x00FF,
];

/// Windows-1250 (Central European), upper half.
const CP1250: [u16; 128] = [
    0x20AC, 0x0081, 0x201A, 0x0083, 0x201E, 0x2026, 0x2020, 0x2021, 0x0088, 0x2030, 0x0160, 0x2039,
    0x015A, 0x0164, 0x017D, 0x0179, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x0098, 0x2122, 0x0161, 0x203A, 0x015B, 0x0165, 0x017E, 0x017A, 0x00A0, 0x02C7, 0x02D8, 0x0141,
    0x00A4, 0x0104, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x015E, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x017B,
    0x00B0, 0x00B1, 0x02DB, 0x0142, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x0105, 0x015F, 0x00BB,
    0x013D, 0x02DD, 0x013E, 0x017C, 0x0154, 0x00C1, 0x00C2, 0x0102, 0x00C4, 0x0139, 0x0106, 0x00C7,
    0x010C, 0x00C9, 0x0118, 0x00CB, 0x011A, 0x00CD, 0x00CE, 0x010E, 0x0110, 0x0143, 0x0147, 0x00D3,
    0x00D4, 0x0150, 0x00D6, 0x00D7, 0x0158, 0x016E, 0x00DA, 0x0170, 0x00DC, 0x00DD, 0x0162, 0x00DF,
    0x0155, 0x00E1, 0x00E2, 0x0103, 0x00E4, 0x013A, 0x0107, 0x00E7, 0x010D, 0x00E9, 0x0119, 0x00EB,
    0x011B, 0x00ED, 0x00EE, 0x010F, 0x0111, 0x0144, 0x0148, 0x00F3, 0x00F4, 0x0151, 0x00F6, 0x00F7,
    0x0159, 0x016F, 0x00FA, 0x0171, 0x00FC, 0x00FD, 0x0163, 0x02D9,
];

/// Windows-1251 (Cyrillic), upper half.
const CP1251: [u16; 128] = [
    0x0402, 0x0403, 0x201A, 0x0453, 0x201E, 0x2026, 0x2020, 0x2021, 0x20AC, 0x2030, 0x0409, 0x2039,
    0x040A, 0x040C, 0x040B, 0x040F, 0x0452, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x0098, 0x2122, 0x0459, 0x203A, 0x045A, 0x045C, 0x045B, 0x045F, 0x00A0, 0x040E, 0x045E, 0x0408,
    0x00A4, 0x0490, 0x00A6, 0x00A7, 0x0401, 0x00A9, 0x0404, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x0407,
    0x00B0, 0x00B1, 0x0406, 0x0456, 0x0491, 0x00B5, 0x00B6, 0x00B7, 0x0451, 0x2116, 0x0454, 0x00BB,
    0x0458, 0x0405, 0x0455, 0x0457, 0x0410, 0x0411, 0x0412, 0x0413, 0x0414, 0x0415, 0x0416, 0x0417,
    0x0418, 0x0419, 0x041A, 0x041B, 0x041C, 0x041D, 0x041E, 0x041F, 0x0420, 0x0421, 0x0422, 0x0423,
    0x0424, 0x0425, 0x0426, 0x0427, 0x0428, 0x0429, 0x042A, 0x042B, 0x042C, 0x042D, 0x042E, 0x042F,
    0x0430, 0x0431, 0x0432, 0x0433, 0x0434, 0x0435, 0x0436, 0x0437, 0x0438, 0x0439, 0x043A, 0x043B,
    0x043C, 0x043D, 0x043E, 0x043F, 0x0440, 0x0441, 0x0442, 0x0443, 0x0444, 0x0445, 0x0446, 0x0447,
    0x0448, 0x0449, 0x044A, 0x044B, 0x044C, 0x044D, 0x044E, 0x044F,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::InputFormat;

    fn convert(rtf: &str) -> DoclingDocument {
        let src = SourceDocument::from_bytes("t.rtf", InputFormat::Rtf, rtf.as_bytes().to_vec());
        RtfBackend.convert(&src).unwrap()
    }

    #[test]
    fn paragraphs_and_inline_formatting() {
        let doc = convert(r"{\rtf1\ansi Plain {\b bold} and \i slanted\i0  text.\par}");
        assert_eq!(
            doc.nodes,
            vec![Node::Paragraph {
                text: "Plain **bold** and *slanted* text.".into()
            }]
        );
    }

    #[test]
    fn headings_from_stylesheet_and_outline() {
        let doc = convert(
            r"{\rtf1\ansi{\stylesheet{\s0 Normal;}{\s1\b heading 1;}}\pard\s1\b Title\b0\par \pard\outlinelevel1 Sub\par \pard Body\par}",
        );
        assert_eq!(
            doc.nodes,
            vec![
                Node::Heading {
                    level: 1,
                    text: "**Title**".into()
                },
                Node::Heading {
                    level: 2,
                    text: "Sub".into()
                },
                Node::Paragraph {
                    text: "Body".into()
                },
            ]
        );
    }

    #[test]
    fn table_rows_and_cells() {
        let doc = convert(
            r"{\rtf1\ansi\trowd\intbl a\cell b\cell\row \trowd\intbl 1\cell 2\cell\row \pard After\par}",
        );
        let Node::Table(t) = &doc.nodes[0] else {
            panic!("expected a table, got {:?}", doc.nodes)
        };
        assert_eq!(t.rows, vec![vec!["a", "b"], vec!["1", "2"]]);
        assert_eq!(
            doc.nodes[1],
            Node::Paragraph {
                text: "After".into()
            }
        );
    }

    #[test]
    fn lists_from_listtext_markers() {
        let doc = convert(
            r"{\rtf1\ansi\pard{\listtext \'b7\tab}First\par\pard{\listtext \'b7\tab}Second\par\pard{\listtext 1.\tab}Num\par}",
        );
        assert!(matches!(
            &doc.nodes[0],
            Node::ListItem { ordered: false, first_in_list: true, text, .. } if text == "First"
        ));
        assert!(matches!(
            &doc.nodes[1],
            Node::ListItem { ordered: false, first_in_list: false, text, .. } if text == "Second"
        ));
        assert!(matches!(
            &doc.nodes[2],
            Node::ListItem { ordered: true, number: 1, text, .. } if text == "Num"
        ));
    }

    #[test]
    fn unicode_escapes_and_codepages() {
        // \uN with a \'3f fallback that must be skipped; cp1252 \'e9 = é.
        let doc = convert(
            r"{\rtf1\ansi\ansicpg1252 caf\'e9 \u1055\'3f\u1088\'3f\u1080\'3f\u1074\'3f\u1077\'3f\u1090\'3f\par}",
        );
        assert_eq!(
            doc.nodes,
            vec![Node::Paragraph {
                text: "café Привет".into()
            }]
        );
    }

    #[test]
    fn skips_furniture_destinations() {
        let doc = convert(
            r"{\rtf1\ansi{\fonttbl{\f0 Arial;}}{\colortbl;\red0\green0\blue0;}{\info{\title secret}}{\*\generator Word}Visible\par}",
        );
        assert_eq!(
            doc.nodes,
            vec![Node::Paragraph {
                text: "Visible".into()
            }]
        );
    }

    #[test]
    fn field_keeps_result_drops_instruction() {
        let doc = convert(
            r#"{\rtf1\ansi{\field{\*\fldinst HYPERLINK "http://x"}{\fldrslt docling.rs}} rules\par}"#,
        );
        assert_eq!(
            doc.nodes,
            vec![Node::Paragraph {
                text: "docling.rs rules".into()
            }]
        );
    }

    #[test]
    fn decodes_embedded_png_picture() {
        // A 1×1 PNG, hex-encoded the way Word embeds it.
        let png_hex = "89504e470d0a1a0a0000000d494844520000000100000001080600000\
                       01f15c4890000000d49444154789c626001000000ffff03000006000\
                       557bfabd40000000049454e44ae426082";
        let rtf = format!(r"{{\rtf1\ansi{{\pict\pngblip\picw1\pich1 {png_hex}}}\par Text\par}}");
        let doc = convert(&rtf);
        let Node::Picture {
            image: Some(img), ..
        } = &doc.nodes[0]
        else {
            panic!("expected a picture, got {:?}", doc.nodes)
        };
        assert_eq!(img.mimetype, "image/png");
        assert_eq!((img.width, img.height), (1, 1));
        assert_eq!(&img.data[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn rejects_non_rtf() {
        let src = SourceDocument::from_bytes("t.rtf", InputFormat::Rtf, b"not rtf at all".to_vec());
        assert!(RtfBackend.convert(&src).is_err());
    }
}
