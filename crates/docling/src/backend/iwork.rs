//! Apple iWork input (issue #213): Pages / Numbers / Keynote.
//!
//! Modern iWork documents (Pages 5+/Numbers 3+/Keynote 6+, i.e. everything
//! since 2013) are zip packages whose `Index/*.iwa` members hold
//! Snappy-framed Protocol Buffer streams — the IWA format. Apple publishes no
//! schema; the message/field numbers below follow the reverse engineering in
//! the `numbers-parser` and `keynote-parser` projects (MIT), which track the
//! format across iWork releases. Only a handful of archive types matter for
//! content, so the stream is walked with a generic wire-format reader —
//! no generated protobuf code, no schema dependency.
//!
//! v1 extracts **text**, per the issue's phasing:
//! - `.pages` — the `BODY` text storage as paragraphs (then any text boxes);
//! - `.key` — slide text boxes as paragraphs, in package order;
//! - `.numbers` — sheets as headings, each table's name plus its shared
//!   string-table entries (the cells' text) as a list. Full grid
//!   reconstruction needs the tile b-tree and is a follow-up.
//!
//! The wire walk is defensive throughout: unknown fields are skipped, short
//! buffers end the walk, and a package with no extractable text converts to
//! an empty document rather than erroring (a blank presentation is not a
//! failure).

use std::collections::HashMap;

use crate::backend::ooxml::Package;
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;
use docling_core::{DoclingDocument, Node};

/// TSWP.StorageArchive — every piece of rich text in any iWork app.
const TYPE_TEXT_STORAGE: u32 = 2001;
/// TN.SheetArchive — a Numbers sheet (type 2 in the Numbers namespace; the
/// per-app namespaces reuse small ids, so these are only consulted for the
/// matching flavor).
const TYPE_TN_SHEET: u32 = 2;
/// TN.DocumentArchive (Numbers root).
const TYPE_TN_DOCUMENT: u32 = 1;
/// TST.TableInfoArchive — a table drawable, references the model.
const TYPE_TST_TABLE_INFO: u32 = 6000;
/// TST.TableModelArchive — table name + data store.
const TYPE_TST_TABLE_MODEL: u32 = 6001;
/// TST.TableDataList — shared per-table value lists (strings, formats, …).
const TYPE_TST_DATA_LIST: u32 = 6005;

/// `TSWP.StorageArchive.KindType` values this backend distinguishes.
const KIND_BODY: u64 = 0;
const KIND_HEADER: u64 = 1;
const KIND_FOOTNOTE: u64 = 2;
const KIND_NOTE: u64 = 4;
const KIND_CELL: u64 = 5;

/// One decoded IWA archive: object identifier, message type of its first
/// message, and that message's payload.
struct Archive {
    id: u64,
    ty: u32,
    payload: Vec<u8>,
}

pub struct IworkBackend;

impl DeclarativeBackend for IworkBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let mut pkg = Package::open(&source.bytes)
            .ok_or_else(|| ConversionError::Parse("iwork: not a zip package".into()))?;

        // Deterministic package order: the root Document.iwa first (it holds
        // the Pages body / the app's root archive), then the remaining .iwa
        // members sorted; slide files sort numerically so `Slide2` precedes
        // `Slide10`.
        // A zipped *package* (the pre-single-file "bundle" layout, or a
        // user-zipped bundle directory) nests the IWA members inside an
        // Index.zip member — unwrap it and read that archive instead.
        if !pkg.names().any(|n| n.ends_with(".iwa")) {
            let inner = pkg
                .names()
                .find(|n| *n == "Index.zip" || n.ends_with("/Index.zip"))
                .map(str::to_string);
            if let Some(inner) = inner {
                if let Some(bytes) = pkg.read_bytes(&inner) {
                    pkg = Package::open(&bytes).ok_or_else(|| {
                        ConversionError::Parse("iwork: Index.zip is not a zip".into())
                    })?;
                }
            }
        }
        // Master slides only carry layout placeholder text ("Title Text",
        // "Body Level One", …) — skipping their members keeps Keynote output
        // to what the deck actually says. `contains` rather than a prefix
        // match: some producers nest the package under an extra directory.
        let mut names: Vec<String> = pkg
            .names()
            .filter(|n| n.ends_with(".iwa") && n.contains("Index/"))
            .filter(|n| !n.contains("Index/MasterSlide"))
            .map(str::to_string)
            .collect();
        if names.is_empty() {
            return Err(ConversionError::Parse(
                "iwork: no Index/*.iwa members — pre-2013 iWork documents (index.xml) \
                 are not supported"
                    .into(),
            ));
        }
        names.sort_by_key(|n| (!n.ends_with("Index/Document.iwa"), natural_key(n)));

        let mut archives: Vec<Archive> = Vec::new();
        for name in &names {
            let Some(bytes) = pkg.read_bytes(name) else {
                continue;
            };
            // A malformed member is skipped, not fatal: the other members
            // still carry content.
            if let Ok(stream) = decode_iwa(&bytes) {
                parse_archives(&stream, &mut archives);
            }
        }

        if docling_core::env::debug_enabled() {
            let mut hist: HashMap<u32, usize> = HashMap::new();
            for a in &archives {
                *hist.entry(a.ty).or_default() += 1;
            }
            let mut counts: Vec<_> = hist.into_iter().collect();
            counts.sort();
            docling_core::debug_log!("iwork: archive types {counts:?}");
        }
        let mut doc = DoclingDocument::new(&source.name);
        let flavor = Flavor::of(source.format);
        match flavor {
            Flavor::Numbers => convert_numbers(&archives, &mut doc),
            Flavor::Pages | Flavor::Keynote => convert_textual(flavor, &archives, &mut doc),
        }
        Ok(doc)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Flavor {
    Pages,
    Numbers,
    Keynote,
}

impl Flavor {
    fn of(format: crate::InputFormat) -> Self {
        match format {
            crate::InputFormat::Numbers => Flavor::Numbers,
            crate::InputFormat::Keynote => Flavor::Keynote,
            _ => Flavor::Pages,
        }
    }
}

/// `Slide10` after `Slide2`: compare path components with embedded numbers
/// expanded to their numeric value.
fn natural_key(name: &str) -> Vec<(u64, String)> {
    let mut key = Vec::new();
    let mut digits = String::new();
    let mut text = String::new();
    for c in name.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            if !digits.is_empty() {
                key.push((
                    digits.parse().unwrap_or(u64::MAX),
                    std::mem::take(&mut text),
                ));
                digits.clear();
            }
            text.push(c);
        }
    }
    key.push((digits.parse().unwrap_or(u64::MAX), text));
    key
}

// --- IWA container ----------------------------------------------------------

/// Un-frame and decompress one `.iwa` member: a sequence of
/// `[type: u8 = 0][length: u24 LE][raw Snappy block]` chunks (Apple frames
/// Snappy itself — this is not the standard Snappy stream format).
fn decode_iwa(bytes: &[u8]) -> Result<Vec<u8>, ConversionError> {
    let mut out = Vec::with_capacity(bytes.len() * 3);
    let mut pos = 0usize;
    let mut snappy = snap::raw::Decoder::new();
    while pos + 4 <= bytes.len() {
        let len = u32::from_le_bytes([bytes[pos + 1], bytes[pos + 2], bytes[pos + 3], 0]) as usize;
        let ty = bytes[pos];
        pos += 4;
        let Some(block) = bytes.get(pos..pos + len) else {
            return Err(ConversionError::Parse("iwork: truncated IWA chunk".into()));
        };
        pos += len;
        match ty {
            0 => out.extend_from_slice(
                &snappy
                    .decompress_vec(block)
                    .map_err(|e| ConversionError::Parse(format!("iwork: snappy: {e}")))?,
            ),
            // Type 1 (uncompressed) has never been observed in the wild but
            // is trivial to honor.
            1 => out.extend_from_slice(block),
            other => {
                return Err(ConversionError::Parse(format!(
                    "iwork: unknown IWA chunk type {other}"
                )))
            }
        }
    }
    Ok(out)
}

/// Split the decompressed stream into archives:
/// `[varint length][TSP.ArchiveInfo][payload per MessageInfo.length]…`.
/// Only each archive's first message is kept — follow-on messages are
/// auxiliary (object-level undo state and the like).
fn parse_archives(mut stream: &[u8], out: &mut Vec<Archive>) {
    while !stream.is_empty() {
        let Some((info_len, rest)) = read_varint(stream) else {
            return;
        };
        let Some(info) = rest.get(..info_len as usize) else {
            return;
        };
        let after = &rest[info_len as usize..];

        // TSP.ArchiveInfo: field 1 = identifier, field 2 = repeated MessageInfo.
        let mut id = 0u64;
        let mut first = true;
        let mut first_ty = 0u32;
        let mut first_len = 0usize;
        let mut total_len = 0usize;
        for (field, value) in Fields::new(info) {
            match (field, value) {
                (1, Value::Varint(v)) => id = v,
                (2, Value::Bytes(mi)) => {
                    // TSP.MessageInfo: field 1 = type, field 3 = length.
                    let (mut ty, mut len) = (0u32, 0usize);
                    for (f, v) in Fields::new(mi) {
                        match (f, v) {
                            (1, Value::Varint(t)) => ty = t as u32,
                            (3, Value::Varint(l)) => len = l as usize,
                            _ => {}
                        }
                    }
                    if first {
                        first_ty = ty;
                        first_len = len;
                        first = false;
                    }
                    total_len += len;
                }
                _ => {}
            }
        }
        let Some(payload) = after.get(..first_len) else {
            return;
        };
        out.push(Archive {
            id,
            ty: first_ty,
            payload: payload.to_vec(),
        });
        let Some(next) = after.get(total_len..) else {
            return;
        };
        stream = next;
    }
}

// --- protobuf wire walking --------------------------------------------------

enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    #[allow(dead_code)]
    Fixed32(u32),
    #[allow(dead_code)]
    Fixed64(u64),
}

/// Iterator over a message's `(field number, value)` pairs. Malformed input
/// ends the iteration — never panics, never reads past the buffer.
struct Fields<'a>(&'a [u8]);

impl<'a> Fields<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Fields(buf)
    }
}

impl<'a> Iterator for Fields<'a> {
    type Item = (u32, Value<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        let (tag, rest) = read_varint(self.0)?;
        let field = (tag >> 3) as u32;
        let value = match tag & 7 {
            0 => {
                let (v, rest) = read_varint(rest)?;
                self.0 = rest;
                Value::Varint(v)
            }
            1 => {
                let v = u64::from_le_bytes(rest.get(..8)?.try_into().ok()?);
                self.0 = &rest[8..];
                Value::Fixed64(v)
            }
            2 => {
                let (len, rest) = read_varint(rest)?;
                let bytes = rest.get(..len as usize)?;
                self.0 = &rest[len as usize..];
                Value::Bytes(bytes)
            }
            5 => {
                let v = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?);
                self.0 = &rest[4..];
                Value::Fixed32(v)
            }
            // Groups (3/4) predate protobuf 2 and never appear in IWA.
            _ => return None,
        };
        Some((field, value))
    }
}

fn read_varint(buf: &[u8]) -> Option<(u64, &[u8])> {
    let mut value = 0u64;
    for (i, &b) in buf.iter().enumerate().take(10) {
        value |= u64::from(b & 0x7f) << (7 * i as u32);
        if b & 0x80 == 0 {
            return Some((value, &buf[i + 1..]));
        }
    }
    None
}

/// `TSP.Reference`: field 1 = the referenced archive's identifier.
fn reference(bytes: &[u8]) -> Option<u64> {
    Fields::new(bytes).find_map(|(f, v)| match (f, v) {
        (1, Value::Varint(id)) => Some(id),
        _ => None,
    })
}

// --- content extraction -----------------------------------------------------

/// A `TSWP.StorageArchive` payload → (kind, its text blocks).
/// Fields: 1 = kind (default TEXTBOX), 3 = repeated string text.
fn storage_text(payload: &[u8]) -> (u64, Vec<String>) {
    let mut kind = 3; // KindType TEXTBOX is the proto default
    let mut texts = Vec::new();
    for (f, v) in Fields::new(payload) {
        match (f, v) {
            (1, Value::Varint(k)) => kind = k,
            (3, Value::Bytes(b)) => {
                if let Ok(s) = std::str::from_utf8(b) {
                    texts.push(s.to_string());
                }
            }
            _ => {}
        }
    }
    (kind, texts)
}

/// Split a storage's text blocks into clean paragraphs: newline-separated,
/// attachment/placeholder control characters removed, blanks dropped.
fn paragraphs(texts: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for block in texts {
        for line in block.split(['\n', '\u{2029}']) {
            let cleaned: String = line
                .chars()
                // U+FFFC/U+FFFB mark inline attachments; iWork also uses a
                // few private-use sentinels (page number fields etc.).
                .filter(|c| !matches!(c, '\u{FFFC}' | '\u{FFFB}' | '\u{E000}'..='\u{F8FF}'))
                .collect();
            let trimmed = cleaned.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
    }
    out
}

/// Pages / Keynote: text storages in package order. The Pages body (kind
/// BODY) leads on its own; text boxes follow, then any tables (their cell
/// text comes from the same TST string tables Numbers uses). Presenter
/// notes, headers and footnotes are excluded.
fn convert_textual(flavor: Flavor, archives: &[Archive], doc: &mut DoclingDocument) {
    let mut boxes: Vec<String> = Vec::new();
    for a in archives {
        if a.ty != TYPE_TEXT_STORAGE {
            continue;
        }
        let (kind, texts) = storage_text(&a.payload);
        match kind {
            KIND_CELL | KIND_NOTE | KIND_HEADER | KIND_FOOTNOTE => continue,
            KIND_BODY if flavor == Flavor::Pages => {
                for p in paragraphs(&texts) {
                    doc.push(Node::Paragraph { text: p });
                }
            }
            _ => boxes.extend(paragraphs(&texts)),
        }
    }
    // Keynote packages carry each layout's placeholder text once per master,
    // per layout and per slide; repeating a paragraph the reader has already
    // seen adds nothing, so text boxes dedup globally (first occurrence wins,
    // package order preserved).
    let mut seen = std::collections::HashSet::new();
    for p in boxes {
        if seen.insert(p.clone()) {
            doc.push(Node::Paragraph { text: p });
        }
    }
    // Tables placed on pages/slides: same TST model + string table as Numbers.
    let by_id: HashMap<u64, &Archive> = archives.iter().map(|a| (a.id, a)).collect();
    for model in archives.iter().filter(|a| a.ty == TYPE_TST_TABLE_MODEL) {
        emit_table(model, &by_id, doc);
    }
}

/// Numbers: document → sheets → table drawables → models → shared string
/// tables. Sheet and table names become headings; the string table holds the
/// cells' text (insertion-keyed — sorted by key for a stable, roughly
/// row-major order).
fn convert_numbers(archives: &[Archive], doc: &mut DoclingDocument) {
    let by_id: HashMap<u64, &Archive> = archives.iter().map(|a| (a.id, a)).collect();

    // TN.DocumentArchive field 1 = repeated sheet references.
    let sheets: Vec<u64> = archives
        .iter()
        .filter(|a| a.ty == TYPE_TN_DOCUMENT)
        .flat_map(|a| {
            Fields::new(&a.payload)
                .filter_map(|(f, v)| match (f, v) {
                    (1, Value::Bytes(b)) => reference(b),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .collect();

    for sheet_id in sheets {
        let Some(sheet) = by_id.get(&sheet_id).filter(|a| a.ty == TYPE_TN_SHEET) else {
            continue;
        };
        // TN.SheetArchive: field 1 = name, field 2 = repeated drawable refs.
        let mut name = String::new();
        let mut drawables = Vec::new();
        for (f, v) in Fields::new(&sheet.payload) {
            match (f, v) {
                (1, Value::Bytes(b)) => name = String::from_utf8_lossy(b).into_owned(),
                (2, Value::Bytes(b)) => drawables.extend(reference(b)),
                _ => {}
            }
        }
        if !name.trim().is_empty() {
            doc.push(Node::Heading {
                level: 1,
                text: name.trim().to_string(),
            });
        }
        for id in drawables {
            let Some(info) = by_id.get(&id).filter(|a| a.ty == TYPE_TST_TABLE_INFO) else {
                continue;
            };
            // TST.TableInfoArchive field 2 = table model reference.
            let model = Fields::new(&info.payload).find_map(|(f, v)| match (f, v) {
                (2, Value::Bytes(b)) => reference(b),
                _ => None,
            });
            let Some(model) = model.and_then(|id| by_id.get(&id)) else {
                continue;
            };
            if model.ty != TYPE_TST_TABLE_MODEL {
                continue;
            }
            emit_table(model, &by_id, doc);
        }
    }
}

/// One `TST.TableModelArchive`: heading from `table_name` (field 8), cells
/// from the data lists behind `base_data_store` (field 4) — plain strings in
/// `stringTable` (field 4), rich-text cells (Keynote/Pages tables) in
/// `rich_text_table` (field 17), whose entries reference CELL text storages.
fn emit_table(model: &Archive, by_id: &HashMap<u64, &Archive>, doc: &mut DoclingDocument) {
    let mut table_name = String::new();
    let mut string_table = None;
    let mut rich_table = None;
    for (f, v) in Fields::new(&model.payload) {
        match (f, v) {
            (8, Value::Bytes(b)) => table_name = String::from_utf8_lossy(b).into_owned(),
            (4, Value::Bytes(store)) => {
                for (sf, sv) in Fields::new(store) {
                    match (sf, sv) {
                        (4, Value::Bytes(b)) => string_table = reference(b),
                        (17, Value::Bytes(b)) => rich_table = reference(b),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if !table_name.trim().is_empty() {
        doc.push(Node::Heading {
            level: 2,
            text: table_name.trim().to_string(),
        });
    }
    let mut entries: Vec<(u64, String)> = Vec::new();
    if let Some(list) = string_table.and_then(|id| by_id.get(&id)) {
        collect_list_entries(list, by_id, &mut entries);
    }
    if let Some(list) = rich_table.and_then(|id| by_id.get(&id)) {
        collect_list_entries(list, by_id, &mut entries);
    }
    entries.sort_by_key(|(k, _)| *k);
    for (i, (_, text)) in entries.into_iter().enumerate() {
        doc.push(Node::ListItem {
            ordered: false,
            number: 0,
            first_in_list: i == 0,
            text,
            level: 0,
            marker: None,
            location: None,
            dclx: None,
            href: None,
            layer: None,
        });
    }
}

/// Read a `TST.TableDataList`'s text entries: field 1 = listType, field 3 =
/// entries; ListEntry: field 1 = key, field 3 = string (STRING lists),
/// field 9 = reference to a CELL text storage (RICH_TEXT_PAYLOAD lists).
fn collect_list_entries(
    list: &Archive,
    by_id: &HashMap<u64, &Archive>,
    out: &mut Vec<(u64, String)>,
) {
    if list.ty != TYPE_TST_DATA_LIST {
        return;
    }
    for (f, v) in Fields::new(&list.payload) {
        if let (3, Value::Bytes(entry)) = (f, v) {
            let (mut key, mut s, mut rich) = (0u64, None, None);
            for (ef, ev) in Fields::new(entry) {
                match (ef, ev) {
                    (1, Value::Varint(k)) => key = k,
                    (3, Value::Bytes(b)) => s = std::str::from_utf8(b).ok().map(str::to_string),
                    (9, Value::Bytes(b)) => rich = reference(b),
                    _ => {}
                }
            }
            if s.is_none() {
                if let Some(storage) = rich.and_then(|id| by_id.get(&id)) {
                    if storage.ty == TYPE_TEXT_STORAGE {
                        let (_, texts) = storage_text(&storage.payload);
                        let joined = paragraphs(&texts).join(" ");
                        if !joined.is_empty() {
                            s = Some(joined);
                        }
                    }
                }
            }
            if let Some(s) = s {
                let t = s.trim();
                if !t.is_empty() {
                    out.push((key, t.to_string()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_and_fields_walk_defensively() {
        assert_eq!(read_varint(&[0x96, 0x01]), Some((150, &[][..])));
        assert_eq!(read_varint(&[0x80]), None); // truncated continuation
                                                // field 1 varint 5, field 2 bytes "ab"
        let msg = [0x08, 0x05, 0x12, 0x02, b'a', b'b'];
        let items: Vec<u32> = Fields::new(&msg).map(|(f, _)| f).collect();
        assert_eq!(items, vec![1, 2]);
        // Truncated length-delimited field ends the walk cleanly.
        let bad = [0x12, 0x0A, b'x'];
        assert_eq!(Fields::new(&bad).count(), 0);
    }

    #[test]
    fn natural_order_sorts_slides_numerically() {
        let mut names = vec!["Index/Slide10.iwa", "Index/Slide2.iwa", "Index/Slide1.iwa"];
        names.sort_by_key(|n| natural_key(n));
        assert_eq!(
            names,
            vec!["Index/Slide1.iwa", "Index/Slide2.iwa", "Index/Slide10.iwa"]
        );
    }
}
