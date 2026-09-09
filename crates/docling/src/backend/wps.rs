//! Microsoft Works word-processor backend (`.wps`) — a docling.rs extension
//! (#216); docling has no Works reader (Python reaches it only via
//! LibreOffice's libwps import filter), and the format is documented mainly
//! by that library, whose structures this port follows.
//!
//! Two generations share one shape — a text stream plus *formatting
//! descriptor pages* (FDP) that map text positions to character formats:
//!
//! - **Works 2.x (DOS), 3.0 and 4.x (Windows)** (`WPS4` in libwps): a 256-byte
//!   header whose second byte is `0xFE` (first byte 1 = DOS 2.x, 4 = Works 3,
//!   6 = Works 4); text limits at `0x1A` (four `i32`: text start, end of
//!   header, end of footer, end of text), the file length at `0x2A`, then
//!   `(u32 offset, u16 length)` entries for `BTEC`/`BTEP` (character and
//!   paragraph PLCs), header/footer, links, footnotes (`FTNp`/`FTNd`),
//!   bookmarks and fonts. The Windows versions wrap that stream in an OLE
//!   container as the `MN0` stream (`MM` marks a Works for Macintosh file).
//!   Text is 8-bit (CP 850 for DOS, CP 1252 for Windows) with control codes:
//!   `0x0A` paragraph end, `0x0B`/`0x0D` line ends, `0x0C` page break, `0x09`
//!   tab, `0x01`–`0x08`/`0x0E`/`0x0F` fields and objects, `0x11`/`0x12`
//!   non-breaking hyphen/space, `0x1F` optional hyphen. `BTEC` is a PLC —
//!   `n+1` `u32` text positions then `n` `u16` page numbers (× 0x80) — of
//!   FDPC pages: 128 bytes ending in the descriptor count, `count+1` `u32`
//!   limits, `count` `u8` offsets to the property blobs (`u8 size-1`, then
//!   flags: bit 0 bold, 1 italic, 2 strikeout; underline, size, sub/superscript
//!   bytes follow).
//! - **Works 5 (2000), 6–9** (`WPS8`): an OLE `CONTENTS` stream starting
//!   `CHNKINK` (5) or `CHNKWKS` (6+); at `0x18` a chained index of
//!   24-byte entries (`name`, id, type, offset, length) naming the zones:
//!   `TEXT` (UTF-16LE), `STRS` (a PLC of text zones typed 1 main, 2/3
//!   foot/endnote, 5 table cells and other frames, 6 header, 7 footer),
//!   `BTEC`→`FDPC` pages (`u16 count`, `u16`, `count+1` `u32` limits,
//!   `count` `u16` blob offsets; blobs `u16 size` + tagged records: 0x02
//!   bold, 0x03 italic, 0x0F super/subscript, 0x10 strikeout, 0x1E
//!   underline, 0x00 special-character kind). Text codes: `0x0D` paragraph
//!   end, `0x0A` line end, `0x0C`/`0x0E` page/column break, `0x09` tab,
//!   `0x1E`/`0x1F` non-breaking hyphen/space, `0x23` a note/field marker
//!   when the run is special, `0xFFFC` an object anchor.
//!
//! Output: paragraphs of runs (bold/italic/strikeout in Markdown; underline,
//! sub/superscript in the DocLang inline runs) from the main text zone, then
//! the foot/endnotes' text as trailing paragraphs; header/footer zones are
//! dropped as page furniture. Works tables (Works 5+ keeps cell text in
//! type-5 zones laid out by an `MCLD` structure) are not rebuilt: their cell
//! text follows the body as paragraphs. Works spreadsheets/databases
//! (`.wks`/`.wdb`, `FF 00 02` / `FF 54` streams) and Works for Macintosh are
//! refused with a targeted error.

use crate::backend::cfb::CompoundFile;
use crate::backend::doc::cp1252;
use crate::backend::markdown::escape_text;
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;
use docling_core::{inline_paragraph_node, DoclingDocument, InlineRun, Script};

pub struct WpsBackend;

impl DeclarativeBackend for WpsBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let bytes = &source.bytes;
        let name = &source.name;
        if CompoundFile::detect(bytes) {
            let cfb = CompoundFile::open(bytes)
                .ok_or_else(|| ConversionError::Parse("wps: unreadable OLE container".into()))?;
            if let Some(mn0) = cfb.stream("MN0") {
                if cfb
                    .stream("MM")
                    .is_some_and(|mm| mm.starts_with(&[0x44, 0x4E]))
                {
                    return Err(ConversionError::Parse(
                        "wps: Microsoft Works for Macintosh document (not supported)".into(),
                    ));
                }
                if mn0.starts_with(&[0xFF, 0x54]) {
                    return Err(ConversionError::Parse(
                        "wps: Microsoft Works database (.wdb), not a word-processor document"
                            .into(),
                    ));
                }
                return wps4(&mn0, name);
            }
            if let Some(contents) = cfb.stream("CONTENTS") {
                if contents.starts_with(b"CHNKWKS") || contents.starts_with(b"CHNKINK") {
                    return wps8(&contents, name);
                }
                return Err(ConversionError::Parse(
                    "wps: CONTENTS stream is not a Works word-processor document".into(),
                ));
            }
            return Err(ConversionError::Parse(
                "wps: OLE container without a Works document stream (no MN0 / CONTENTS)".into(),
            ));
        }
        match bytes.first().copied().zip(bytes.get(1).copied()) {
            Some((v, 0xFE)) if v <= 7 => wps4(bytes, name),
            Some((0xFF, 0x00)) if bytes.get(2) == Some(&2) => Err(ConversionError::Parse(
                "wps: Microsoft Works spreadsheet (.wks), not a word-processor document \
                 (convert it as .wks)"
                    .into(),
            )),
            Some((0xFF | 0x20, 0x54)) => Err(ConversionError::Parse(
                "wps: Microsoft Works database (.wdb), not a word-processor document".into(),
            )),
            _ => Err(ConversionError::Parse(
                "wps: not a Microsoft Works document (no Works header or OLE container)".into(),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Shared: formatting runs → paragraphs.

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Fmt {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    script: Script,
    /// The run is a field/note/object marker, not text (WPS8's special kind).
    special: bool,
}

struct Run {
    text: String,
    fmt: Fmt,
}

/// A character-format change at a text position (an FDPC descriptor).
struct Fod {
    pos: usize,
    fmt: Fmt,
}

struct Builder {
    doc: DoclingDocument,
    runs: Vec<Run>,
}

impl Builder {
    fn new(name: &str) -> Self {
        Self {
            doc: DoclingDocument::new(name),
            runs: Vec::new(),
        }
    }

    fn text(&mut self, s: &str, fmt: Fmt) {
        if s.is_empty() {
            return;
        }
        match self.runs.last_mut() {
            Some(r) if r.fmt == fmt => r.text.push_str(s),
            _ => self.runs.push(Run {
                text: s.to_string(),
                fmt,
            }),
        }
    }

    fn ch(&mut self, c: char, fmt: Fmt) {
        let mut buf = [0u8; 4];
        self.text(c.encode_utf8(&mut buf), fmt);
    }

    /// A line end inside a paragraph: one space unless the line already ends
    /// in whitespace.
    fn soft_break(&mut self, fmt: Fmt) {
        let ends_ws = self
            .runs
            .last()
            .and_then(|r| r.text.chars().last())
            .is_none_or(char::is_whitespace);
        if !ends_ws {
            self.text(" ", fmt);
        }
    }

    fn end_paragraph(&mut self) {
        let runs = std::mem::take(&mut self.runs);
        let md = runs_markdown(&runs);
        if md.is_empty() {
            return;
        }
        self.doc
            .push(inline_paragraph_node(md, runs_inline(&runs), false));
    }

    fn finish(mut self) -> DoclingDocument {
        self.end_paragraph();
        self.doc
    }
}

/// Markdown for a paragraph's runs: escaped text with bold/italic/strike
/// markers around each run's non-blank core (whitespace stays outside the
/// markers), tabs as spaces, trimmed.
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

/// The format in force at `pos` given the sorted descriptors (the last one
/// starting at or before `pos`; default before the first).
fn fmt_at(fods: &[Fod], pos: usize) -> Fmt {
    match fods.partition_point(|f| f.pos <= pos) {
        0 => Fmt::default(),
        n => fods[n - 1].fmt,
    }
}

fn u16le(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32le(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn i32le(d: &[u8], at: usize) -> Option<i32> {
    u32le(d, at).map(|v| v as i32)
}

// ---------------------------------------------------------------------------
// Works 2/3/4 (WPS4).

/// A `(begin, end)` byte range of the stream.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Zone {
    begin: usize,
    end: usize,
}

impl Zone {
    fn valid(&self) -> bool {
        self.end > self.begin
    }
}

fn wps4(d: &[u8], name: &str) -> Result<DoclingDocument, ConversionError> {
    if d.len() < 0x100 {
        return Err(ConversionError::Parse(
            "wps: Works document stream shorter than its 256-byte header".into(),
        ));
    }
    let dos = d[0] < 4;
    // Text limits: text start, end of header, end of footer, end of text.
    // Zones may be empty or missing (-1); libwps' reading, rounding the
    // start up to the end of the header.
    let mut lim = [0i32; 4];
    for (k, l) in lim.iter_mut().enumerate() {
        *l = i32le(d, 0x1A + 4 * k).unwrap_or(-1);
    }
    let mut last = if lim[0] < 0x100 {
        0x100usize
    } else {
        lim[0] as usize
    };
    let mut zones = [Zone::default(); 3];
    let mut ok = true;
    let (mut text_begin, mut text_end) = (None, None);
    for i in 0..3 {
        let new_pos = lim[i + 1];
        let zone = Zone {
            begin: last,
            end: new_pos.max(0) as usize,
        };
        if new_pos >= 0 && new_pos as usize >= last {
            last = new_pos as usize;
        }
        if !zone.valid() || zone.begin < 0x100 {
            if new_pos != 0x100 && new_pos != -1 {
                ok = false;
            }
            continue;
        }
        text_begin.get_or_insert(zone.begin);
        text_end = Some(zone.end);
        zones[i] = zone;
    }
    let (Some(tb), Some(te)) = (text_begin, text_end) else {
        return Err(ConversionError::Parse(
            "wps: Works document has no text zone".into(),
        ));
    };
    let text = Zone {
        begin: tb,
        end: te.min(d.len()),
    };
    // Header and footer are page furniture and are dropped; only the main
    // zone is read (all of the text when the limits looked inconsistent).
    let main = if ok { zones[2] } else { text };
    // Named entries after the length word: (u32 offset, u16 length) each.
    let entry = |k: usize| -> Option<Zone> {
        let at = 0x2E + 6 * k;
        let begin = u32le(d, at)? as usize;
        let len = u16le(d, at + 4)? as usize;
        (begin > 0 && len > 0 && begin + len <= d.len()).then_some(Zone {
            begin,
            end: begin + len,
        })
    };
    let btec = entry(0);
    let ftnp = entry(5);
    let ftnd = entry(6);

    let fods = wps4_fdpc(d, text, btec);
    // Footnote definitions live inside the main zone (`FTNd` positions,
    // relative to the text start); they are cut out of the body and appended
    // after it.
    let notes = wps4_footnotes(d, text, ftnp, ftnd);
    let mut b = Builder::new(name);
    let mut pos = main.begin.max(text.begin);
    let end = main.end.min(text.end);
    let mut sorted_notes = notes.clone();
    sorted_notes.sort_by_key(|z| z.begin);
    for note in &sorted_notes {
        if note.begin >= pos && note.end <= end {
            wps4_text(
                d,
                Zone {
                    begin: pos,
                    end: note.begin,
                },
                &fods,
                dos,
                &mut b,
            );
            pos = note.end;
        }
    }
    if pos < end {
        wps4_text(d, Zone { begin: pos, end }, &fods, dos, &mut b);
    }
    b.end_paragraph();
    for note in &sorted_notes {
        wps4_text(d, *note, &fods, dos, &mut b);
        b.end_paragraph();
    }
    Ok(b.finish())
}

/// The character-format descriptors of a WPS4 stream: `BTEC` (a PLC of page
/// numbers) names the 128-byte FDPC pages; without a usable one the pages
/// are found by hand — they follow the text, 128-byte aligned, each opening
/// with the position where the previous one stopped (libwps' fallback).
fn wps4_fdpc(d: &[u8], text: Zone, btec: Option<Zone>) -> Vec<Fod> {
    let mut pages: Vec<usize> = Vec::new();
    if let Some(plc) = btec {
        // n+1 positions (u32), then n u16 values; the last position is the
        // text end.
        let len = plc.end - plc.begin;
        let mut n = 0;
        while (n + 1) * 4 + n * 2 + 4 <= len {
            n += 1;
            if (n + 1) * 4 + n * 2 == len {
                break;
            }
        }
        if n > 0 && (n + 1) * 4 + n * 2 == len {
            for k in 0..n {
                if let Some(page) = u16le(d, plc.begin + (n + 1) * 4 + 2 * k) {
                    let at = usize::from(page) * 0x80;
                    if at + 0x80 <= d.len() {
                        pages.push(at);
                    }
                }
            }
        }
    }
    if pages.is_empty() {
        let mut at = ((text.end + 127) >> 7) * 0x80;
        let mut last = text.begin;
        while at + 0x80 <= d.len() {
            let count = usize::from(d[at + 0x7F]);
            if count == 0 || 5 * count + 4 > 0x80 {
                break;
            }
            if u32le(d, at).map(|v| v as usize) != Some(last) {
                break;
            }
            let Some(new_pos) = u32le(d, at + 4 * count).map(|v| v as usize) else {
                break;
            };
            if new_pos < last || new_pos > text.end {
                break;
            }
            pages.push(at);
            if new_pos == text.end {
                break;
            }
            last = new_pos;
            at += 0x80;
        }
    }
    let mut fods = Vec::new();
    for page in pages {
        let p = &d[page..page + 0x80];
        let count = usize::from(p[0x7F]);
        if count == 0 || 5 * count + 4 > 0x80 {
            continue;
        }
        for k in 0..count {
            let Some(pos) = u32le(p, 4 * k).map(|v| v as usize) else {
                break;
            };
            let off = usize::from(p[4 * (count + 1) + k]);
            let fmt = if off == 0 {
                Fmt::default()
            } else {
                wps4_font(p, off)
            };
            fods.push(Fod {
                pos: if pos == 0 { text.begin } else { pos },
                fmt,
            });
        }
    }
    fods.sort_by_key(|f| f.pos);
    fods
}

/// A WPS4 character property blob at `off` in its page: `u8 size-1`, then
/// `flags` (bit 0 bold, 1 italic, 2 strikeout), `what`, font id, underline,
/// size, sub/superscript offset (signed), colors, link id — as many as the
/// size covers.
fn wps4_font(p: &[u8], off: usize) -> Fmt {
    let Some(&size) = p.get(off) else {
        return Fmt::default();
    };
    let end = (off + 1 + usize::from(size) + 1).min(p.len());
    let field = |k: usize| p.get(off + 1 + k).filter(|_| off + 1 + k < end).copied();
    let flags = field(0).unwrap_or(0);
    Fmt {
        bold: flags & 0x01 != 0,
        italic: flags & 0x02 != 0,
        strike: flags & 0x04 != 0,
        underline: field(3).is_some_and(|u| u != 0),
        script: match field(5).map(|v| v as i8) {
            Some(v) if v > 0 => Script::Super,
            Some(v) if v < 0 => Script::Sub,
            _ => Script::Baseline,
        },
        special: field(1).is_some_and(|w| w & 0x02 != 0),
    }
}

/// The footnote text ranges (`FTNd`: n+1 positions relative to the text
/// start, `FTNp` the matching reference positions, 12 bytes of data each).
fn wps4_footnotes(d: &[u8], text: Zone, ftnp: Option<Zone>, ftnd: Option<Zone>) -> Vec<Zone> {
    let (Some(ftnp), Some(ftnd)) = (ftnp, ftnd) else {
        return Vec::new();
    };
    // FTNp: (n+1) u32 + n × 12 bytes → n
    let plen = ftnp.end - ftnp.begin;
    if plen < 4 + 4 + 12 || (plen - 4) % 16 != 0 {
        return Vec::new();
    }
    let n = (plen - 4) / 16;
    // FTNd: (n+1) u32 of definition positions
    if ftnd.end - ftnd.begin != 4 * (n + 1) {
        return Vec::new();
    }
    let mut defs: Vec<usize> = (0..=n)
        .filter_map(|k| u32le(d, ftnd.begin + 4 * k))
        .map(|v| text.begin + v as usize)
        .collect();
    if defs.len() != n + 1 {
        return Vec::new();
    }
    defs.sort_unstable();
    let mut out = Vec::new();
    for w in defs.windows(2) {
        let z = Zone {
            begin: w[0],
            end: w[1].min(text.end),
        };
        if z.valid() && z.begin >= text.begin {
            out.push(z);
        }
    }
    out
}

/// Decode a WPS4 text range into the builder.
fn wps4_text(d: &[u8], zone: Zone, fods: &[Fod], dos: bool, b: &mut Builder) {
    let end = zone.end.min(d.len());
    let mut i = zone.begin;
    while i < end {
        let c = d[i];
        let fmt = fmt_at(fods, i);
        i += 1;
        match c {
            0x0A | 0x0C => b.end_paragraph(),
            0x0B | 0x0D => {
                // 0x0D 0x0A pairs: the 0x0A ends the paragraph
                if c == 0x0B || d.get(i) != Some(&0x0A) {
                    b.soft_break(fmt);
                }
            }
            0x09 => b.text("\t", fmt),
            0x11 => b.ch('\u{2011}', fmt),
            0x12 => b.ch('\u{a0}', fmt),
            0x00..=0x1F => {} // fields, objects, note markers, optional hyphens
            0x20..=0x7E => b.ch(c as char, fmt),
            0x7F => {}
            0xCA if dos => b.ch('\u{a0}', fmt),
            _ => b.ch(
                if dos {
                    CP850[usize::from(c - 0x80)]
                } else {
                    cp1252(c)
                },
                fmt,
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Works 5+ (WPS8).

struct Entry {
    name: [u8; 4],
    typ: [u8; 4],
    zone: Zone,
}

fn wps8(d: &[u8], name: &str) -> Result<DoclingDocument, ConversionError> {
    let entries = wps8_index(d);
    let find = |n: &[u8; 4], t: Option<&[u8; 4]>| -> Vec<&Entry> {
        entries
            .iter()
            .filter(|e| &e.name == n && t.is_none_or(|t| &e.typ == t))
            .collect()
    };
    let text = find(b"TEXT", None)
        .first()
        .map(|e| e.zone)
        .ok_or_else(|| ConversionError::Parse("wps: Works document without a TEXT zone".into()))?;
    let text = Zone {
        begin: text.begin.min(d.len()),
        end: text.end.min(d.len()),
    };
    // Text zones and their kinds (STRS); the whole text is the main zone
    // when there is none.
    let mut zones: Vec<(i32, Zone)> = find(b"STRS", Some(b"PLC "))
        .first()
        .map(|e| wps8_strs(d, e.zone, text))
        .unwrap_or_default();
    if zones.is_empty() {
        zones.push((1, text));
    }
    // Character formats: BTEC (a PLC whose values are FDPC offsets) or every
    // FDPC entry.
    let fdpc_entries = find(b"FDPC", None);
    let mut pages: Vec<Zone> = Vec::new();
    for btec in find(b"BTEC", Some(b"PLC ")) {
        for off in wps8_plc_values(d, btec.zone) {
            if let Some(e) = fdpc_entries.iter().find(|e| e.zone.begin == off) {
                pages.push(e.zone);
            }
        }
    }
    if pages.is_empty() {
        pages = fdpc_entries.iter().map(|e| e.zone).collect();
    }
    let mut fods: Vec<Fod> = Vec::new();
    for page in pages {
        wps8_fdpc(d, page, text, &mut fods);
    }
    fods.sort_by_key(|f| f.pos);

    let mut b = Builder::new(name);
    // main text first, then notes, then the other frames (table cells,
    // text boxes); header/footer (6/7) are furniture.
    for wanted in [&[1][..], &[2, 3], &[5]] {
        for (kind, zone) in zones.iter().filter(|(k, _)| wanted.contains(k)) {
            wps8_text(d, *zone, &fods, *kind == 1, &mut b);
            b.end_paragraph();
        }
    }
    Ok(b.finish())
}

/// The header index: at 0x18 a chain of tables (`u16`, `u16 count`, `u32
/// next`), each holding `cch`-byte entries — normally 24: name, id, two
/// words, type, offset, length (longer ones carry a trailing string).
fn wps8_index(d: &[u8]) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut total = u16le(d, 0x0C).unwrap_or(0) as usize;
    let mut table = 0x18usize;
    let mut guard = 0;
    while total > 0 && guard < 64 {
        guard += 1;
        let Some(n_local) = u16le(d, table + 2) else {
            break;
        };
        let Some(next) = u32le(d, table + 4) else {
            break;
        };
        if n_local > 0x20 {
            break;
        }
        let mut pos = table + 8;
        let mut left = usize::from(n_local);
        while left > 0 && total > 0 {
            let Some(cch) = u16le(d, pos).map(usize::from) else {
                return out;
            };
            if cch < 10 || pos + cch > d.len() {
                return out;
            }
            if cch >= 0x18 {
                let e = &d[pos + 2..pos + 0x18];
                let name: [u8; 4] = [e[0], e[1], e[2], e[3]];
                let plausible = name
                    .iter()
                    .all(|&c| c == 0 || c == b' ' || (41..=90).contains(&c));
                if plausible {
                    let typ: [u8; 4] = [e[10], e[11], e[12], e[13]];
                    let begin = u32::from_le_bytes([e[14], e[15], e[16], e[17]]) as usize;
                    let len = u32::from_le_bytes([e[18], e[19], e[20], e[21]]) as usize;
                    if begin + len <= d.len() {
                        out.push(Entry {
                            name,
                            typ,
                            zone: Zone {
                                begin,
                                end: begin + len,
                            },
                        });
                    }
                }
            }
            pos += cch;
            left -= 1;
            total -= 1;
        }
        if next == 0xFFFF_FFFF || (next as usize) < table {
            break;
        }
        table = next as usize;
    }
    out
}

/// A WPS8 PLC: `u32 n`, `u32 data size`, 4 flag bytes, `n+1` `u32`
/// positions, then `n` data blocks. Returns the positions (raw) and the
/// blocks.
fn wps8_plc(d: &[u8], z: Zone) -> Option<(Vec<u32>, Vec<&[u8]>)> {
    let n = u32le(d, z.begin)? as usize;
    let data_sz = u32le(d, z.begin + 4)? as usize;
    let pos_at = z.begin + 16;
    if pos_at + 4 * (n + 1) > z.end || n > 1 << 20 {
        return None;
    }
    let positions: Vec<u32> = (0..=n).filter_map(|k| u32le(d, pos_at + 4 * k)).collect();
    let mut blocks = Vec::new();
    let mut at = pos_at + 4 * (n + 1);
    let sz = if data_sz > 0 && at + n * data_sz <= z.end {
        data_sz
    } else if n > 0 && (z.end - at).is_multiple_of(n) {
        (z.end - at) / n
    } else {
        0
    };
    for _ in 0..n {
        blocks.push(&d[at..(at + sz).min(z.end)]);
        at += sz;
    }
    Some((positions, blocks))
}

/// The `n` `u32` values of a constant-size PLC (BTEC: FDPC page offsets).
fn wps8_plc_values(d: &[u8], z: Zone) -> Vec<usize> {
    let Some((_, blocks)) = wps8_plc(d, z) else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter(|b| b.len() >= 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
        .collect()
}

/// The STRS PLC: consecutive zone lengths (in UTF-16 units) from the text
/// start; each block is tagged data whose record `0` (type 0x22) is the zone
/// kind.
fn wps8_strs(d: &[u8], z: Zone, text: Zone) -> Vec<(i32, Zone)> {
    let Some((lengths, blocks)) = wps8_plc(d, z) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut at = text.begin;
    let mut last_kind = 1;
    for (k, block) in blocks.iter().enumerate() {
        let len = 2 * lengths.get(k).copied().unwrap_or(0) as usize;
        let zone = Zone {
            begin: at,
            end: (at + len).min(text.end),
        };
        at += len;
        let kind = wps8_records(block)
            .into_iter()
            .find(|r| r.id == 0 && r.typ == 0x22)
            .map(|r| r.value as i32)
            .unwrap_or(last_kind);
        last_kind = kind;
        if zone.valid() {
            out.push((kind, zone));
        }
    }
    out
}

/// One tagged record of WPS8 block data.
struct Rec {
    id: u8,
    typ: u8,
    value: i64,
}

/// WPS8 block data: a `u16` header value, then records `u16 (type<<8|id)`
/// whose type's high nibble says how much follows — 0: nothing, 1: `u16`
/// (0x12: `u8`+pad), 2: `i32` (0x2a: 4 chars + `i32`), 8: `u16 size` +
/// nested bytes (skipped). Records must come in increasing id order; a bad
/// one ends the list.
fn wps8_records(block: &[u8]) -> Vec<Rec> {
    let mut out = Vec::new();
    let mut i = 2;
    let mut prev = -1i32;
    while i + 2 <= block.len() {
        let key = u16::from_le_bytes([block[i], block[i + 1]]);
        i += 2;
        let typ = (key >> 8) as u8;
        let id = (key & 0xFF) as u8;
        if typ & 5 != 0 || (i32::from(id)) < prev {
            break;
        }
        prev = i32::from(id);
        let value: i64 = match typ >> 4 {
            0 => 0,
            1 => {
                if i + 2 > block.len() {
                    break;
                }
                let v = if typ == 0x12 {
                    i64::from(block[i])
                } else {
                    i64::from(u16::from_le_bytes([block[i], block[i + 1]]))
                };
                i += 2;
                v
            }
            2 => {
                if typ == 0x2A {
                    i += 4;
                }
                if i + 4 > block.len() {
                    break;
                }
                let v = i64::from(i32::from_le_bytes([
                    block[i],
                    block[i + 1],
                    block[i + 2],
                    block[i + 3],
                ]));
                i += 4;
                v
            }
            8 => {
                if i + 2 > block.len() {
                    break;
                }
                let extra = usize::from(u16::from_le_bytes([block[i], block[i + 1]]));
                i += extra;
                0
            }
            _ => break,
        };
        out.push(Rec { id, typ, value });
    }
    out
}

/// A WPS8 FDPC page: `u16 count`, `u16`, `count+1` `u32` limits, `count`
/// `u16` blob offsets; each blob `u16 size` + block data with the font
/// records.
fn wps8_fdpc(d: &[u8], page: Zone, text: Zone, fods: &mut Vec<Fod>) {
    let p = &d[page.begin.min(d.len())..page.end.min(d.len())];
    let Some(count) = u16le(p, 0).map(usize::from) else {
        return;
    };
    if count == 0 || 8 + 6 * count > p.len() {
        return;
    }
    for k in 0..count {
        let Some(pos) = u32le(p, 4 + 4 * k).map(|v| v as usize) else {
            return;
        };
        let Some(off) = u16le(p, 4 + 4 * (count + 1) + 2 * k).map(usize::from) else {
            return;
        };
        let pos = if pos == 0 { text.begin } else { pos };
        if pos > text.end {
            return;
        }
        let fmt = if off == 0 {
            Fmt::default()
        } else {
            let size = u16le(p, off).map(usize::from).unwrap_or(0);
            let end = (off + size).min(p.len());
            if size >= 2 && off + 2 <= end {
                wps8_font(&p[off + 2..end])
            } else {
                Fmt::default()
            }
        };
        fods.push(Fod { pos, fmt });
    }
}

fn wps8_font(block: &[u8]) -> Fmt {
    let mut f = Fmt::default();
    for r in wps8_records(block) {
        let on = r.typ == 0x0A; // a "true" flag record
        match r.id {
            0x00 => f.special = r.value != 0,
            0x02 => f.bold = on,
            0x03 => f.italic = on,
            0x0F => {
                f.script = match r.value {
                    1 => Script::Super,
                    2 => Script::Sub,
                    _ => Script::Baseline,
                }
            }
            0x10 => f.strike = on,
            0x1E => f.underline = true,
            _ => {}
        }
    }
    f
}

/// Decode a UTF-16LE text zone into the builder.
fn wps8_text(d: &[u8], zone: Zone, fods: &[Fod], main: bool, b: &mut Builder) {
    let end = zone.end.min(d.len());
    let mut i = zone.begin;
    while i + 1 < end {
        let fmt = fmt_at(fods, i);
        let v = u16::from_le_bytes([d[i], d[i + 1]]);
        i += 2;
        match v {
            0 => {}
            0x09 => b.text("\t", fmt),
            0x0A => b.soft_break(fmt),
            0x0C | 0x0E => {
                if main {
                    b.end_paragraph();
                } else {
                    b.soft_break(fmt);
                }
            }
            0x0D => b.end_paragraph(),
            0x1E => b.ch('\u{2011}', fmt),
            0x1F => b.ch('\u{a0}', fmt),
            0x23 if fmt.special => {} // footnote / field / object marker
            0xFFFC => {}
            v if v < 28 => {}
            0xD800..=0xDBFF => {
                let Some(low) = u16le(d, i) else {
                    break;
                };
                i += 2;
                if (0xDC00..0xE000).contains(&low) {
                    let cp = 0x10000 + ((u32::from(v) - 0xD800) << 10) + (u32::from(low) - 0xDC00);
                    if let Some(c) = char::from_u32(cp) {
                        b.ch(c, fmt);
                    }
                }
            }
            0xDC00..=0xDFFF => {}
            v => {
                if let Some(c) = char::from_u32(u32::from(v)) {
                    b.ch(c, fmt);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// DOS code page 850 (Works 2.x for DOS text bytes 0x80–0xFF).

const CP850: [char; 128] = [
    '\u{00c7}', '\u{00fc}', '\u{00e9}', '\u{00e2}', '\u{00e4}', '\u{00e0}', '\u{00e5}', '\u{00e7}',
    '\u{00ea}', '\u{00eb}', '\u{00e8}', '\u{00ef}', '\u{00ee}', '\u{00ec}', '\u{00c4}', '\u{00c5}',
    '\u{00c9}', '\u{00e6}', '\u{00c6}', '\u{00f4}', '\u{00f6}', '\u{00f2}', '\u{00fb}', '\u{00f9}',
    '\u{00ff}', '\u{00d6}', '\u{00dc}', '\u{00f8}', '\u{00a3}', '\u{00d8}', '\u{00d7}', '\u{0192}',
    '\u{00e1}', '\u{00ed}', '\u{00f3}', '\u{00fa}', '\u{00f1}', '\u{00d1}', '\u{00aa}', '\u{00ba}',
    '\u{00bf}', '\u{00ae}', '\u{00ac}', '\u{00bd}', '\u{00bc}', '\u{00a1}', '\u{00ab}', '\u{00bb}',
    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2502}', '\u{2524}', '\u{00c1}', '\u{00c2}', '\u{00c0}',
    '\u{00a9}', '\u{2563}', '\u{2551}', '\u{2557}', '\u{255d}', '\u{00a2}', '\u{00a5}', '\u{2510}',
    '\u{2514}', '\u{2534}', '\u{252c}', '\u{251c}', '\u{2500}', '\u{253c}', '\u{00e3}', '\u{00c3}',
    '\u{255a}', '\u{2554}', '\u{2569}', '\u{2566}', '\u{2560}', '\u{2550}', '\u{256c}', '\u{00a4}',
    '\u{00f0}', '\u{00d0}', '\u{00ca}', '\u{00cb}', '\u{00c8}', '\u{0131}', '\u{00cd}', '\u{00ce}',
    '\u{00cf}', '\u{2518}', '\u{250c}', '\u{2588}', '\u{2584}', '\u{00a6}', '\u{00cc}', '\u{2580}',
    '\u{00d3}', '\u{00df}', '\u{00d4}', '\u{00d2}', '\u{00f5}', '\u{00d5}', '\u{00b5}', '\u{00fe}',
    '\u{00de}', '\u{00da}', '\u{00db}', '\u{00d9}', '\u{00fd}', '\u{00dd}', '\u{00af}', '\u{00b4}',
    '\u{00ad}', '\u{00b1}', '\u{2017}', '\u{00be}', '\u{00b6}', '\u{00a7}', '\u{00f7}', '\u{00b8}',
    '\u{00b0}', '\u{00a8}', '\u{00b7}', '\u{00b9}', '\u{00b3}', '\u{00b2}', '\u{25a0}', '\u{00a0}',
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::InputFormat;

    fn convert(bytes: Vec<u8>) -> Result<DoclingDocument, ConversionError> {
        WpsBackend.convert(&SourceDocument::from_bytes(
            "t.wps",
            InputFormat::Works,
            bytes,
        ))
    }

    fn md(doc: &DoclingDocument) -> String {
        doc.export_to_markdown()
    }

    /// A minimal Works 2.x DOS stream: 256-byte header, text at 0x100, one
    /// FDPC page on the next 128-byte boundary, then the BTEC PLC naming it.
    fn wps4_stream(text: &[u8], bold: std::ops::Range<usize>) -> Vec<u8> {
        let text_begin = 0x100usize;
        let text_end = text_begin + text.len();
        let page = ((text_end + 127) >> 7) * 0x80;
        let plc = page + 0x80;
        let eof = plc + 4 + 4 + 2;
        let mut v = vec![0u8; eof];
        v[0] = 1;
        v[1] = 0xFE;
        for (k, lim) in [-1i32, -1, text_begin as i32, text_end as i32]
            .iter()
            .enumerate()
        {
            v[0x1A + 4 * k..0x1E + 4 * k].copy_from_slice(&lim.to_le_bytes());
        }
        v[0x2A..0x2E].copy_from_slice(&(eof as u32).to_le_bytes());
        // BTEC entry: offset, length
        v[0x2E..0x32].copy_from_slice(&(plc as u32).to_le_bytes());
        v[0x32..0x34].copy_from_slice(&10u16.to_le_bytes());
        v[text_begin..text_end].copy_from_slice(text);
        // FDPC page: 3 descriptors (plain, bold over `bold`, plain again)
        let p = &mut v[page..page + 0x80];
        p[0x7F] = 3;
        let limits = [
            text_begin,
            text_begin + bold.start,
            text_begin + bold.end,
            text_end,
        ];
        for (k, pos) in limits.iter().enumerate() {
            p[4 * k..4 * k + 4].copy_from_slice(&(*pos as u32).to_le_bytes());
        }
        p[16] = 0; // plain: default properties
        p[17] = 0x20; // bold blob at 0x20
        p[18] = 0; // plain again
        p[0x20] = 1; // size - 1
        p[0x21] = 0x01; // flags: bold
                        // BTEC PLC: positions [begin, end], page number
        v[plc..plc + 4].copy_from_slice(&(text_begin as u32).to_le_bytes());
        v[plc + 4..plc + 8].copy_from_slice(&(text_end as u32).to_le_bytes());
        v[plc + 8..plc + 10].copy_from_slice(&((page / 0x80) as u16).to_le_bytes());
        v
    }

    #[test]
    fn works2_dos_text_codes_and_bold() {
        // "Hello " plain, "World" bold, CRLF paragraph end, a CP850 byte
        // (0x82 = é), tab, soft return 0x0B, hard hyphen 0x11.
        let text = b"Hello World\r\ncaf\x82\tx\x0by\x11z".to_vec();
        let doc = convert(wps4_stream(&text, 6..11)).unwrap();
        assert_eq!(md(&doc), "Hello **World**\n\ncaf\u{e9} x y\u{2011}z\n");
    }

    #[test]
    fn works2_header_without_text_zone_is_an_error() {
        let mut v = vec![0u8; 0x100];
        v[0] = 1;
        v[1] = 0xFE;
        for k in 0..4 {
            v[0x1A + 4 * k..0x1E + 4 * k].copy_from_slice(&(-1i32).to_le_bytes());
        }
        let e = convert(v).unwrap_err().to_string();
        assert!(e.contains("no text zone"), "{e}");
    }

    /// A minimal Works 6 `CONTENTS` stream (as it would sit in the OLE
    /// container): magic, entry count at 0x0C, one index table at 0x18 with a
    /// TEXT entry and an FDPC entry, then the data.
    fn wps8_contents(text: &str, bold_from_char: usize) -> Vec<u8> {
        let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let text_begin = 0x18 + 8 + 2 * 24;
        let text_end = text_begin + utf16.len();
        let fdpc_begin = (text_end + 3) & !3;
        // page: count=2, unk, limits ×3, offsets ×2, blob
        let blob_off = 4 + 12 + 4;
        let blob: Vec<u8> = {
            let mut b = vec![];
            b.extend_from_slice(&6u16.to_le_bytes()); // size incl. itself
            b.extend_from_slice(&0u16.to_le_bytes()); // block header value
            b.extend_from_slice(&0x0A02u16.to_le_bytes()); // bold = true
            b
        };
        let fdpc_len = blob_off + blob.len();
        let mut v = vec![0u8; fdpc_begin + fdpc_len];
        v[..8].copy_from_slice(b"CHNKWKS ");
        v[0x0C..0x0E].copy_from_slice(&2u16.to_le_bytes());
        v[0x18 + 2..0x18 + 4].copy_from_slice(&2u16.to_le_bytes());
        v[0x18 + 4..0x18 + 8].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        let entry = |v: &mut Vec<u8>, at: usize, name: &[u8; 4], begin: usize, len: usize| {
            v[at..at + 2].copy_from_slice(&24u16.to_le_bytes());
            v[at + 2..at + 6].copy_from_slice(name);
            v[at + 12..at + 16].copy_from_slice(name);
            v[at + 16..at + 20].copy_from_slice(&(begin as u32).to_le_bytes());
            v[at + 20..at + 24].copy_from_slice(&(len as u32).to_le_bytes());
        };
        entry(&mut v, 0x20, b"TEXT", text_begin, utf16.len());
        entry(&mut v, 0x38, b"FDPC", fdpc_begin, fdpc_len);
        v[text_begin..text_end].copy_from_slice(&utf16);
        let p = &mut v[fdpc_begin..];
        p[0..2].copy_from_slice(&2u16.to_le_bytes());
        let bold_at = text_begin + 2 * bold_from_char;
        for (k, pos) in [text_begin, bold_at, text_end].iter().enumerate() {
            p[4 + 4 * k..8 + 4 * k].copy_from_slice(&(*pos as u32).to_le_bytes());
        }
        p[16..18].copy_from_slice(&0u16.to_le_bytes());
        p[18..20].copy_from_slice(&(blob_off as u16).to_le_bytes());
        p[blob_off..blob_off + blob.len()].copy_from_slice(&blob);
        v
    }

    #[test]
    fn works6_contents_paragraphs_and_bold() {
        // paragraph end 0x0D, line end 0x0A (space), nbsp 0x1F, bold tail
        let doc = wps8(&wps8_contents("One\rTwo\nthree\u{1f}x Bold", 15), "t").unwrap();
        assert_eq!(md(&doc), "One\n\nTwo three\u{a0}x **Bold**\n");
    }

    #[test]
    fn refuses_other_works_files() {
        let e = convert(b"\xff\x00\x02\x00\x04\x04".to_vec())
            .unwrap_err()
            .to_string();
        assert!(e.contains("spreadsheet"), "{e}");
        let e = convert(b"\xff\x54\x02\x00".to_vec())
            .unwrap_err()
            .to_string();
        assert!(e.contains("database"), "{e}");
        let e = convert(b"plain text file".to_vec())
            .unwrap_err()
            .to_string();
        assert!(e.contains("not a Microsoft Works document"), "{e}");
    }

    #[test]
    fn libreoffice_fixtures_parse() {
        // LibreOffice's libwps smoke files are near-empty documents; they
        // must at least parse and detect their generation.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/works/sources");
        for f in [
            "works2_dos.wps",
            "works3.wps",
            "works45.wps",
            "works5.wps",
            "works6.wps",
        ] {
            let bytes = std::fs::read(dir.join(f)).unwrap();
            convert(bytes).unwrap_or_else(|e| panic!("{f}: {e}"));
        }
    }
}
