//! Lotus Word Pro (`.lwp`, Word Pro 97 – Millennium 9.8), a docling.rs
//! extension (#216); docling has no reader, LibreOffice imports it with its
//! `lotuswordpro` filter, whose object model this follows.
//!
//! A Word Pro file is an object store: after the 16-byte `WordPro` prefix,
//! an old-style object header introduces the file header (the file
//! revision, the root index offset); the index — a root over leaf pages,
//! object ids delta-compressed, a time table that "indexed" ids point
//! into — maps every object id to its offset. Each object has a variable
//! header (its type, flag bits that size the version, reference count and
//! data length, and mark the data run-length compressed) and its data,
//! little-endian.
//!
//! The text lives in *stories* (`VO_STORY`): a story names the layout it
//! flows into — the page (the body), a header or footer, a table cell, a
//! frame, a footnote — and the head of its paragraph list; each paragraph
//! (`VO_PARA`) links to the next and holds its *fribs*, the runs of text,
//! tabs, breaks and fields. A text frib is code-page text that may switch
//! to UTF-16 at a NUL byte and back at a NUL word ("packed unicode").
//!
//! The body story comes first, then the other content stories (table
//! cells, frames) as paragraphs in object order — the table grid and the
//! frame anchors are not rebuilt; headers, footers and footnotes are
//! furniture. A paragraph whose style is named `Heading N` is a heading;
//! a revision marked deleted is left out (the accepted view). Numbering,
//! bullets and character formatting (kept in style objects and the font
//! table) are not read. Word Pro 96 files and the Bento-packaged,
//! PKWARE-imploded form are refused by name.

use std::collections::{HashMap, HashSet};

use docling_core::{DoclingDocument, Node};

use crate::backend::markdown::escape_text;
use crate::backend::styled::{Para, Style};
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;

pub struct LwpBackend;

/// Whether `bytes` open like an (unpacked) Word Pro file.
pub(crate) fn looks_like_lwp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"WordPro\0") && bytes.get(16..20) == Some(b"LWP7")
}

/// Every object's data is read relative to this base.
const BASE: usize = 0x10;
/// LibreOffice's cap on one object's (decompressed) data.
const MAX_OBJECT: usize = 0xff00;
/// Bound on index pages and paragraph chains — far past a real file.
const MAX_WALK: usize = 1 << 20;

// Object types (`VO_*`).
const VO_PARA: u16 = 0;
const VO_STORY: u16 = 13;
const VO_FNCELLLAYOUT: u16 = 61;
const VO_FOOTNOTELAYOUT: u16 = 65;
const VO_ENDNOTELAYOUT: u16 = 138;
const VO_HEADERLAYOUT: u16 = 139;
const VO_FOOTERLAYOUT: u16 = 140;
const VO_PAGELAYOUT: u16 = 143;
const VO_CONTONLAYOUT: u16 = 145;
const VO_CONTFROMLAYOUT: u16 = 146;
const VO_DROPCAPLAYOUT: u16 = 217;
const VO_ROOTLEAFOBJINDEX: u16 = 0xfffb;
const VO_ROOTOBJINDEX: u16 = 0xfffc;
const VO_OBJINDEX: u16 = 0xfffd;
const VO_LEAFOBJINDEX: u16 = 0xfffe;

/// An object id: a time stamp and a serial number.
type Id = (u32, u16);
const NULL: Id = (0, 0);

/// A little-endian cursor that reads zeros past its end, as LibreOffice's
/// object stream does; `short` records that it did.
struct Cursor<'a> {
    d: &'a [u8],
    o: usize,
    short: bool,
}

impl<'a> Cursor<'a> {
    fn new(d: &'a [u8]) -> Self {
        Cursor {
            d,
            o: 0,
            short: false,
        }
    }

    fn take(&mut self, n: usize) -> &'a [u8] {
        let start = self.o.min(self.d.len());
        let end = self.o.saturating_add(n).min(self.d.len());
        if end - start < n {
            self.short = true;
        }
        self.o = self.o.saturating_add(n);
        &self.d[start..end]
    }

    fn u8(&mut self) -> u8 {
        self.take(1).first().copied().unwrap_or(0)
    }

    fn u16(&mut self) -> u16 {
        let b = self.take(2);
        if b.len() == 2 {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            0
        }
    }

    fn u32(&mut self) -> u32 {
        let b = self.take(4);
        if b.len() == 4 {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            0
        }
    }

    fn id(&mut self) -> Id {
        (self.u32(), self.u16())
    }

    /// `SkipExtra`: 16-bit words up to a zero one.
    fn skip_extra(&mut self) {
        while !self.short && self.u16() != 0 {}
    }
}

/// LibreOffice's object-data compression: a control byte either stands for
/// a run of zeros (`00xxxxxx`), zeros then literal bytes (`01zzzlll`), or
/// literal bytes (`1xxxxxxx`, `10` with a zero first).
fn decompress(src: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        i += 1;
        match c & 0xc0 {
            0x00 => out.resize(out.len() + (c & 0x3f) as usize + 1, 0),
            0x40 => {
                out.resize(out.len() + ((c & 0x38) >> 3) as usize + 1, 0);
                let n = (c & 7) as usize + 1;
                out.extend_from_slice(src.get(i..i + n)?);
                i += n;
            }
            tag => {
                if tag == 0x80 {
                    out.push(0);
                }
                let n = (c & 0x3f) as usize + 1;
                out.extend_from_slice(src.get(i..i + n)?);
                i += n;
            }
        }
        if out.len() >= MAX_OBJECT {
            return None;
        }
    }
    Some(out)
}

struct File<'a> {
    d: &'a [u8],
    /// The index's time table: an indexed id's time stamp by its 1-based
    /// position.
    times: Vec<u32>,
    offsets: HashMap<Id, u32>,
    /// Index order, for the stories' order.
    order: Vec<Id>,
}

impl<'a> File<'a> {
    /// An id in its indexed form: a time-table position (non-zero) or a
    /// full time stamp, then the serial number.
    fn indexed(&self, c: &mut Cursor) -> Id {
        let i = c.u8() as usize;
        let time = if i == 0 {
            c.u32()
        } else {
            self.times.get(i - 1).copied().unwrap_or(0)
        };
        (time, c.u16())
    }

    /// The object header at `at`: its type and (decompressed) data.
    fn object(&self, at: usize) -> Option<(u16, Vec<u8>)> {
        let mut c = Cursor::new(self.d.get(at..)?);
        let tag = c.u16();
        let flags = c.u8();
        let _id = self.indexed(&mut c);
        match flags & 0x03 {
            1 => {
                c.u8();
            }
            2 => {
                c.u16();
            }
            3 => {
                c.u32();
            }
            _ => {}
        }
        match flags & 0x0c {
            4 => {
                c.u8();
            }
            8 => {
                c.u16();
            }
            _ => {
                c.u32();
            }
        }
        if flags & 0x40 != 0 {
            c.u32();
        }
        let size = match flags & 0x30 {
            0x10 => c.u8() as usize,
            0x20 => c.u16() as usize,
            _ => c.u32() as usize,
        };
        if c.short || size >= MAX_OBJECT {
            return None;
        }
        let start = at + c.o;
        let raw = self.d.get(start..start + size)?;
        let data = if flags & 0x80 != 0 {
            decompress(raw)?
        } else {
            raw.to_vec()
        };
        Some((tag, data))
    }

    fn by_id(&self, id: Id) -> Option<(u16, Vec<u8>)> {
        let at = *self.offsets.get(&id)? as usize;
        self.object(BASE + at)
    }

    /// `count` delta-compressed ids: the first in full, each next one its
    /// predecessor's serial plus `diff + 1` (255 = a full id follows).
    fn key_ids(c: &mut Cursor, count: usize) -> Vec<Id> {
        let mut out: Vec<Id> = Vec::with_capacity(count.min(4096));
        for k in 0..count {
            if c.short {
                break;
            }
            let id = if k == 0 {
                c.id()
            } else {
                match c.u8() {
                    255 => c.id(),
                    diff => {
                        let prev = out[k - 1];
                        (prev.0, prev.1.wrapping_add(diff as u16 + 1))
                    }
                }
            };
            out.push(id);
        }
        out
    }

    fn leaf(&mut self, c: &mut Cursor) {
        let n = c.u16() as usize;
        let ids = Self::key_ids(c, n);
        for id in ids {
            let off = c.u32();
            if self.offsets.insert(id, off).is_none() {
                self.order.push(id);
            }
        }
    }

    fn times(&mut self, c: &mut Cursor) {
        let n = c.u16() as usize;
        for _ in 0..n {
            if c.short {
                break;
            }
            self.times.push(c.u32());
        }
    }

    /// The index: a root with no children (`ROOTLEAF`), or a root over
    /// object-index pages, each over leaf pages (or a leaf itself). The
    /// keys between children name objects too.
    fn read_index(&mut self, at: usize, walked: &mut usize) {
        let Some((tag, data)) = self.object(at) else {
            return;
        };
        let mut c = Cursor::new(&data);
        match tag {
            VO_ROOTLEAFOBJINDEX => {
                self.leaf(&mut c);
                self.times(&mut c);
            }
            VO_ROOTOBJINDEX | VO_OBJINDEX => {
                let keys = c.u16() as usize;
                let leaves = if keys > 0 { keys + 1 } else { 0 };
                let ids = Self::key_ids(&mut c, keys);
                let offs: Vec<u32> = (0..keys).map(|_| c.u32()).collect();
                let children: Vec<u32> = (0..leaves).map(|_| c.u32()).collect();
                if tag == VO_ROOTOBJINDEX {
                    self.times(&mut c);
                }
                for (k, child) in children.into_iter().enumerate() {
                    *walked += 1;
                    if *walked > MAX_WALK {
                        return;
                    }
                    self.read_index(BASE + child as usize, walked);
                    if let (Some(&id), Some(&off)) = (ids.get(k), offs.get(k)) {
                        if self.offsets.insert(id, off).is_none() {
                            self.order.push(id);
                        }
                    }
                }
            }
            VO_LEAFOBJINDEX => self.leaf(&mut c),
            _ => {}
        }
    }
}

/// A paragraph as read: the next one, its style and text, and whether a
/// page break ends it.
struct ParaObj {
    next: Id,
    style: Id,
    text: String,
    page_break: bool,
}

/// Text in a code page (0 = the default, Windows-1252).
fn code_page(bytes: &[u8], cp: u16, latin1: bool) -> String {
    if latin1 {
        return bytes.iter().map(|&b| b as char).collect();
    }
    let label = match cp {
        932 => "shift_jis".to_string(),
        936 => "gbk".to_string(),
        949 => "euc-kr".to_string(),
        950 => "big5".to_string(),
        874 | 1250..=1258 => format!("windows-{cp}"),
        _ => "windows-1252".to_string(),
    };
    let enc =
        encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::WINDOWS_1252);
    enc.decode_without_bom_handling(bytes).0.into_owned()
}

/// Word Pro's "packed unicode": code-page bytes up to a NUL, then UTF-16LE
/// words up to a NUL word, and so on.
fn packed(bytes: &[u8], cp: u16, latin1: bool) -> String {
    if !bytes.contains(&0) {
        return code_page(bytes, cp, latin1);
    }
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let end = bytes[i..]
            .iter()
            .position(|&b| b == 0)
            .map_or(bytes.len(), |p| i + p);
        out.push_str(&code_page(&bytes[i..end], cp, latin1));
        i = end + 1;
        let mut units = Vec::new();
        while i + 1 < bytes.len() {
            let u = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
            i += 2;
            if u == 0 {
                break;
            }
            units.push(u);
        }
        out.push_str(&String::from_utf16_lossy(&units));
    }
    out
}

// Frib types.
const FRIB_TEXT: u8 = 2;
const FRIB_TAB: u8 = 4;
const FRIB_PAGEBREAK: u8 = 5;
const FRIB_LINEBREAK: u8 = 9;
const FRIB_HARDSPACE: u8 = 10;
const FRIB_UNICODE: u8 = 13;
const FRIB_UNICODE3: u8 = 15;

fn read_para(file: &File, data: &[u8]) -> ParaObj {
    let mut c = Cursor::new(data);
    let next = file.indexed(&mut c);
    let _prev = file.indexed(&mut c);
    let flag = c.u8();
    let simple = flag & 1 != 0;
    if !simple {
        c.u32(); // ordinal
        if flag & 2 != 0 {
            // Two notify lists: a head id and its extra words each.
            for _ in 0..2 {
                file.indexed(&mut c);
                c.skip_extra();
            }
        }
    }
    c.u16(); // flags
    let style = file.indexed(&mut c);
    if !simple {
        c.u32(); // hint: width
        c.u32(); // and height
    }
    let _story = file.indexed(&mut c);
    if !simple {
        c.u16(); // level
    }
    let mut text = String::new();
    let mut page_break = false;
    loop {
        let tag = c.u8();
        let kind = tag & 0x3f;
        if kind == 0 || c.short {
            break;
        }
        c.u8(); // editor
        let (mut cp, mut deleted) = (0u16, false);
        if tag & 0x80 != 0 {
            // Modifiers: (tag, length, data) up to a zero tag.
            loop {
                let m = c.u8();
                if m == 0 || c.short {
                    break;
                }
                let len = c.u8() as usize;
                let body = c.take(len);
                match (m, body.len()) {
                    (8, 2) => cp = u16::from_le_bytes([body[0], body[1]]),
                    (2, n) if n >= 1 => deleted = body[0] == 1,
                    _ => {}
                }
            }
        }
        let len = c.u16() as usize;
        let body = c.take(len);
        if deleted {
            continue;
        }
        match kind {
            FRIB_TEXT => text.push_str(&packed(body, cp, tag & 0x40 != 0)),
            FRIB_UNICODE..=FRIB_UNICODE3 => text.push_str(&packed(body, cp, false)),
            FRIB_TAB | FRIB_LINEBREAK | FRIB_HARDSPACE => text.push(' '),
            FRIB_PAGEBREAK => page_break = true,
            _ => {}
        }
    }
    ParaObj {
        next,
        style,
        text,
        page_break,
    }
}

/// What a story's layout makes of its text.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Body,
    Other,
    Furniture(&'static str),
    Skip,
}

fn role(layout: Option<u16>) -> Role {
    match layout {
        Some(VO_PAGELAYOUT) => Role::Body,
        Some(VO_HEADERLAYOUT) => Role::Furniture("page_header"),
        Some(VO_FOOTERLAYOUT) => Role::Furniture("page_footer"),
        Some(VO_FNCELLLAYOUT | VO_FOOTNOTELAYOUT | VO_ENDNOTELAYOUT) => Role::Furniture("footnote"),
        // The "continued on / from" notices are page furniture templates,
        // a drop cap's story repeats its paragraph's first letter; a story
        // with no layout is a numbering or bullet definition.
        Some(VO_CONTONLAYOUT | VO_CONTFROMLAYOUT | VO_DROPCAPLAYOUT) | None => Role::Skip,
        Some(_) => Role::Other,
    }
}

/// A story's layout and the head of its paragraph list.
fn read_story(file: &File, data: &[u8]) -> (Id, Id) {
    let mut c = Cursor::new(data);
    let atom = |c: &mut Cursor| {
        let disk = c.u16() as usize;
        let len = c.u16();
        if len != 0 && disk >= 2 {
            c.take(disk - 2);
        }
    };
    // DLNFVList: next, previous, child head (and tail), parent, name.
    file.indexed(&mut c);
    file.indexed(&mut c);
    if file.indexed(&mut c) != NULL {
        file.indexed(&mut c);
    }
    file.indexed(&mut c);
    atom(&mut c);
    // Content: the associated layouts (the only one, or a list), flags,
    // class name, enumeration links, an optional notify id.
    let only = file.indexed(&mut c);
    let head = file.indexed(&mut c);
    if head != NULL {
        file.indexed(&mut c);
    }
    c.skip_extra();
    c.u16();
    atom(&mut c);
    file.indexed(&mut c);
    file.indexed(&mut c);
    if c.u8() != 0 {
        file.indexed(&mut c);
        c.skip_extra();
    }
    c.skip_extra();
    // Story: the paragraph list's head.
    let paras = file.indexed(&mut c);
    (if only != NULL { only } else { head }, paras)
}

/// A style object's name (it is a DLNFVList: the name follows the links).
fn style_name(file: &File, id: Id) -> Option<String> {
    let (_, data) = file.by_id(id)?;
    let mut c = Cursor::new(&data);
    file.indexed(&mut c);
    file.indexed(&mut c);
    if file.indexed(&mut c) != NULL {
        file.indexed(&mut c);
    }
    file.indexed(&mut c);
    let disk = c.u16() as usize;
    if c.u16() == 0 || disk < 2 {
        return None;
    }
    let name = c.take(disk - 2);
    Some(packed(name, 0, false).trim_end_matches('\0').to_string())
}

fn convert(d: &[u8], name: &str) -> Result<DoclingDocument, ConversionError> {
    if d.starts_with(b"WordPro") && d.get(16..20) != Some(b"LWP7") {
        return Err(ConversionError::Parse(
            "lwp: a Bento-packaged (imploded) Word Pro file — only the unpacked form \
             Word Pro writes by default is read"
                .into(),
        ));
    }
    if !looks_like_lwp(d) {
        return Err(ConversionError::Parse(
            "lwp: not a Lotus Word Pro file".into(),
        ));
    }
    // The file header behind an old-style object header: tag, id,
    // version, reference count, next-version offset and id, size.
    let mut c = Cursor::new(&d[BASE..]);
    c.take(4 + 6 + 4 + 4 + 4 + 4 + 4);
    c.u16(); // application revision
    let revision = c.u16();
    c.take(6);
    c.id(); // the document object
    let root = c.u32() as usize;
    if revision < 0x0b {
        return Err(ConversionError::Parse(format!(
            "lwp: a Word Pro 96 file (file revision {revision}) — only Word Pro 97 and later \
             are read"
        )));
    }
    let mut file = File {
        d,
        times: Vec::new(),
        offsets: HashMap::new(),
        order: Vec::new(),
    };
    let mut walked = 0;
    file.read_index(BASE + root, &mut walked);
    if file.offsets.is_empty() {
        return Err(ConversionError::Parse(
            "lwp: the object index is unreadable".into(),
        ));
    }

    let mut doc = DoclingDocument::new(name);
    let mut body = Vec::new();
    let mut others = Vec::new();
    let mut furniture = Vec::new();
    let mut styles: HashMap<Id, Option<u8>> = HashMap::new();
    for &id in &file.order {
        let Some((VO_STORY, data)) = file.by_id(id) else {
            continue;
        };
        let (layout, head) = read_story(&file, &data);
        let kind = file.by_id(layout).map(|(t, _)| t);
        let role = role(if layout == NULL { None } else { kind });
        if role == Role::Skip {
            continue;
        }
        let mut seen = HashSet::new();
        let mut at = head;
        while at != NULL && seen.insert(at) && seen.len() < MAX_WALK {
            let Some((VO_PARA, data)) = file.by_id(at) else {
                break;
            };
            let para = read_para(&file, &data);
            at = para.next;
            let heading = *styles
                .entry(para.style)
                .or_insert_with(|| heading_level(style_name(&file, para.style)));
            match role {
                Role::Furniture(label) => {
                    let text = para.text.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !text.is_empty() {
                        furniture.push(Node::FurnitureText {
                            label: label.into(),
                            text: escape_text(&text),
                        });
                    }
                }
                Role::Body => body.push((para, heading)),
                _ => others.push((para, heading)),
            }
        }
    }
    for (para, heading) in body.into_iter().chain(others) {
        let text = para.text.split_whitespace().collect::<Vec<_>>().join(" ");
        match heading {
            Some(level) if !text.is_empty() => doc.push(Node::Heading {
                level,
                text: escape_text(&text),
            }),
            _ => {
                let mut p = Para::new();
                p.push(&para.text, Style::default());
                if let Some(node) = p.into_node() {
                    doc.push(node);
                }
            }
        }
        if para.page_break {
            doc.push(Node::PageBreak);
        }
    }
    doc.nodes.extend(furniture);
    Ok(doc)
}

/// `Heading N` → N (1–6), with the German and French SmartMasters'
/// names (`Überschrift N`, `Titre N`).
fn heading_level(name: Option<String>) -> Option<u8> {
    let name = name?;
    let (word, n) = name.trim().rsplit_once(' ')?;
    if !matches!(word, "Heading" | "Überschrift" | "Titre") {
        return None;
    }
    let n: u8 = n.parse().ok()?;
    (1..=9).contains(&n).then_some(n.min(6))
}

impl DeclarativeBackend for LwpBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        convert(&source.bytes, &source.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompression_runs() {
        // Two zeros; one zero then "ab"; the literal "xyz".
        let src = [0x01, 0x41, b'a', b'b', 0xc2, b'x', b'y', b'z'];
        assert_eq!(decompress(&src).unwrap(), b"\0\0\0abxyz");
    }

    #[test]
    fn packed_unicode_switches_at_nul() {
        let bytes = b"Word Pro \0\x19\x20\0\0 97";
        assert_eq!(packed(bytes, 0, false), "Word Pro \u{2019} 97");
        assert_eq!(packed(b"caf\xe9", 0, false), "café");
    }

    #[test]
    fn headings_come_from_style_names() {
        assert_eq!(heading_level(Some("Heading 2".into())), Some(2));
        assert_eq!(heading_level(Some("Default Text".into())), None);
        assert_eq!(heading_level(Some("Überschrift 1".into())), Some(1));
    }

    #[test]
    fn foreign_bytes_are_refused() {
        assert!(convert(b"hello", "t").is_err());
        let mut bento = b"WordPro\0".to_vec();
        bento.resize(32, 0);
        let err = convert(&bento, "t").unwrap_err();
        assert!(err.to_string().contains("Bento"), "{err}");
    }

    /// Every fixture, cut at every few bytes, converts or errors — never
    /// panics.
    #[test]
    fn truncated_files_do_not_panic() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/lwp/sources");
        for entry in std::fs::read_dir(dir).unwrap() {
            let bytes = std::fs::read(entry.unwrap().path()).unwrap();
            for end in (0..bytes.len()).step_by(97) {
                let _ = convert(&bytes[..end], "t");
            }
        }
    }
}
