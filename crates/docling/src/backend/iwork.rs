//! Apple iWork input (issue #213, #318): Pages / Numbers / Keynote.
//!
//! Modern iWork documents (Pages 5+/Numbers 3+/Keynote 6+, i.e. everything
//! since 2013) are zip packages whose `Index/*.iwa` members hold
//! Snappy-framed Protocol Buffer streams — the IWA format. Apple publishes no
//! schema; the message/field numbers below follow the reverse engineering in
//! the `numbers-parser` and `keynote-parser` projects (MIT) and docling's own
//! Pages reader. Only a handful of archive types matter for content, so the
//! stream is walked with a generic wire-format reader — no generated protobuf
//! code, no schema dependency.
//!
//! **Pages is a conformance format** (#318): upstream docling gained a Pages
//! reader in 2.121 (docling#3934) and titles/headings/iWork '09 tables in
//! 2.122 (docling#4031), so `.pages` mirrors `IWorkPagesDocumentBackend`
//! byte-for-byte — both container generations:
//! - Pages 5+ (`Index/*.iwa`): `TP.DocumentArchive` → the *body* text storage
//!   (text boxes, headers, footnotes are not read, as upstream), each
//!   paragraph labelled from its paragraph style ("Title", "Heading N",
//!   "Subheading"), then every `TST` table as a grid, appended after the body;
//! - iWork '09 and earlier (`index.xml`, optionally gzipped): the body
//!   `sf:p` paragraphs (page furniture pruned, template placeholders skipped)
//!   and `sf:tabular-model` tables.
//!
//! Numbers and Keynote remain docling.rs extensions (upstream has no reader),
//! text-level per the original phasing:
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

use crate::backend::markdown::escape_text;
use crate::backend::ooxml::Package;
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;
use docling_core::{DoclingDocument, Node, Table, TableCell};

/// TSWP.StorageArchive — every piece of rich text in any iWork app.
const TYPE_TEXT_STORAGE: u32 = 2001;
/// TP.DocumentArchive — the root object of a Pages document; field 4
/// references the body text storage.
const TYPE_TP_DOCUMENT: u32 = 10000;
/// TSWP.ParagraphStyleArchive — a paragraph style whose `TSS.StyleArchive`
/// super (field 1) carries the human-facing name ("Body", "Heading 1").
const TYPE_TSWP_PARAGRAPH_STYLE: u32 = 2022;
/// TST.Tile — lays a table's cells out into rows (Pages tables).
const TYPE_TST_TILE: u32 = 6002;
/// iWork '09 XML namespaces.
const SF_NS: &str = "http://developer.apple.com/namespaces/sf";
const SFA_NS: &str = "http://developer.apple.com/namespaces/sfa";
/// Decompressed-size ceiling for a legacy `index.xml.gz` (docling's
/// `_MAX_LEGACY_XML_BYTES`): the package size caps only see the stored size.
const MAX_LEGACY_XML_BYTES: u64 = 100 * 1024 * 1024;
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
        let flavor = Flavor::of(source.format);

        // docling's `is_encrypted`: Pages does not set the standard ZIP
        // encryption flag on a password-protected document — it writes a
        // compression method outside the ZIP-defined set — so both signals are
        // checked before any member is decompressed.
        if pkg.any_encrypted() {
            return Err(ConversionError::Parse(
                "iwork: the document is password-protected; docling.rs cannot read \
                 encrypted iWork documents. Remove the password in Pages and save again"
                    .into(),
            ));
        }

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

        // `contains` rather than a prefix match: some producers nest the
        // package under an extra directory.
        let has_iwa = pkg
            .names()
            .any(|n| n.ends_with(".iwa") && n.contains("Index/"));
        let mut doc = DoclingDocument::new(&source.name);
        if !has_iwa {
            if flavor == Flavor::Pages {
                // iWork '09 and earlier: a plain (optionally gzipped) index.xml.
                let legacy = ["index.xml", "index.xml.gz"]
                    .into_iter()
                    .find(|m| pkg.names().any(|n| n == *m));
                if let Some(member) = legacy {
                    convert_pages_legacy(&mut pkg, member, &mut doc)?;
                    return Ok(doc);
                }
                return Err(ConversionError::Parse(
                    "iwork: a ZIP archive, but not a Pages document — it has neither an \
                     Index/ directory nor an index.xml"
                        .into(),
                ));
            }
            return Err(ConversionError::Parse(
                "iwork: no Index/*.iwa members — pre-2013 Numbers/Keynote documents \
                 (index.xml) are not supported"
                    .into(),
            ));
        }
        if flavor == Flavor::Pages {
            convert_pages_iwa(&mut pkg, &mut doc)?;
            return Ok(doc);
        }

        // Deterministic package order: the app's root archive first, then the
        // remaining .iwa members sorted; slide files sort numerically so
        // `Slide2` precedes `Slide10`.
        // Master slides only carry layout placeholder text ("Title Text",
        // "Body Level One", …) — skipping their members keeps Keynote output
        // to what the deck actually says.
        let mut names: Vec<String> = pkg
            .names()
            .filter(|n| n.ends_with(".iwa") && n.contains("Index/"))
            .filter(|n| !n.contains("Index/MasterSlide"))
            .map(str::to_string)
            .collect();
        names.sort_by_key(|n| (!n.ends_with("Index/Document.iwa"), natural_key(n)));

        let mut archives: Vec<Archive> = Vec::new();
        for name in &names {
            let Some(bytes) = pkg.read_bytes(name) else {
                continue;
            };
            // A malformed member is skipped, not fatal: the other members
            // still carry content.
            if let Ok(stream) = decode_iwa(&bytes) {
                parse_archives(&stream, &mut archives, false);
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
///
/// With `all_messages` false only each archive's first message is kept —
/// follow-on messages are auxiliary (object-level undo state and the like).
/// The Pages parity path passes true: docling's `iter_objects` yields every
/// message under the archive's identifier and keys them last-wins, and
/// reproducing that keeps object lookup identical to upstream.
fn parse_archives(mut stream: &[u8], out: &mut Vec<Archive>, all_messages: bool) {
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
        let mut messages: Vec<(u32, usize)> = Vec::new();
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
                    messages.push((ty, len));
                }
                _ => {}
            }
        }
        let mut consumed = 0usize;
        for (i, (ty, len)) in messages.iter().enumerate() {
            let Some(payload) = after.get(consumed..consumed + len) else {
                return;
            };
            if i == 0 || all_messages {
                out.push(Archive {
                    id,
                    ty: *ty,
                    payload: payload.to_vec(),
                });
            }
            consumed += len;
        }
        let Some(next) = after.get(consumed..) else {
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

// --- Pages (conformance format, docling's IWorkPagesDocumentBackend) --------

/// The docling label a Pages paragraph style implies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PagesLabel {
    Text,
    Title,
    /// docling section-header level (1-based, capped at 6).
    Heading(u8),
}

/// docling's `_label_for_style`: Pages names its built-in styles the same way
/// in both container generations ("Title", "Heading 1", "Subheading", "Body"),
/// so one mapping serves the IWA and XML readers. Custom styles are unknown
/// and stay body text; so does an anonymous (ad-hoc formatting) style.
fn label_for_style(style_name: Option<&str>) -> PagesLabel {
    let Some(name) = style_name else {
        return PagesLabel::Text;
    };
    let name = name.trim();
    if name.is_empty() {
        return PagesLabel::Text;
    }
    let lowered = name.to_lowercase();
    if lowered == "title" {
        return PagesLabel::Title;
    }
    if lowered == "subtitle" || lowered == "subheading" {
        return PagesLabel::Heading(2);
    }
    // `^heading\s*(\d+)?$`: a bare "Heading" is the top level — Pages' Layout
    // template pairs it with "Subheading" rather than numbering them.
    if let Some(rest) = lowered.strip_prefix("heading") {
        let digits = rest.trim_start();
        if digits.is_empty() {
            return PagesLabel::Heading(1);
        }
        if digits.chars().all(|c| c.is_ascii_digit()) {
            let level = digits.parse::<u64>().unwrap_or(u64::MAX).min(6) as u8;
            return PagesLabel::Heading(level);
        }
    }
    PagesLabel::Text
}

/// docling's `_clean`: drop U+FFFC (Apple's inline-attachment marker — an
/// image or footnote anchor with no text of its own) and trim.
fn clean_pages_text(text: &str) -> String {
    text.replace('\u{FFFC}', "").trim().to_string()
}

/// Push labelled paragraphs the way docling's `convert` does: a title item,
/// a section header at its level, or body text. Text is escaped like every
/// declarative backend's (docling-core's serializer does it on output).
fn push_pages_paragraphs(paragraphs: Vec<(String, PagesLabel)>, doc: &mut DoclingDocument) {
    for (text, label) in paragraphs {
        let text = escape_text(&text);
        match label {
            PagesLabel::Text => doc.push(Node::Paragraph { text }),
            // `Node::Heading` level 1 is docling's title; a section header of
            // docling level N is our level N + 1.
            PagesLabel::Title => doc.push(Node::Heading { level: 1, text }),
            PagesLabel::Heading(level) => doc.push(Node::Heading {
                level: level.saturating_add(1),
                text,
            }),
        }
    }
}

/// First length-delimited value of `field` (docling's `fields.get(f, [None])[0]`
/// with its `isinstance(..., bytes)` check).
fn first_bytes(payload: &[u8], field: u32) -> Option<&[u8]> {
    Fields::new(payload).find_map(|(f, v)| match v {
        Value::Bytes(b) if f == field => Some(b),
        _ => None,
    })
}

/// First varint value of `field`.
fn first_varint(payload: &[u8], field: u32) -> Option<u64> {
    Fields::new(payload).find_map(|(f, v)| match v {
        Value::Varint(n) if f == field => Some(n),
        _ => None,
    })
}

/// Pages 5+ (`Index/*.iwa`): docling's `_read_iwa_document` + `_iwa_tables`.
///
/// Objects are keyed by identifier over the `.iwa` members in archive order,
/// every message included, later definitions replacing earlier ones (a Python
/// dict keeps the first key's position with the last value) — reproduced so
/// the body lookup and the table order match upstream exactly.
fn convert_pages_iwa(pkg: &mut Package, doc: &mut DoclingDocument) -> Result<(), ConversionError> {
    let names: Vec<String> = pkg
        .names()
        .filter(|n| n.ends_with(".iwa"))
        .map(str::to_string)
        .collect();
    let mut archives: Vec<Archive> = Vec::new();
    for name in &names {
        let Some(bytes) = pkg.read_bytes(name) else {
            continue;
        };
        // Upstream fails the document on a malformed member.
        let stream = decode_iwa(&bytes)?;
        parse_archives(&stream, &mut archives, true);
    }
    let mut order: Vec<u64> = Vec::new();
    let mut by_id: HashMap<u64, &Archive> = HashMap::new();
    for a in &archives {
        if by_id.insert(a.id, a).is_none() {
            order.push(a.id);
        }
    }

    let document = order
        .iter()
        .filter_map(|id| by_id.get(id))
        .find(|a| a.ty == TYPE_TP_DOCUMENT)
        .ok_or_else(|| {
            ConversionError::Parse(
                "iwork: the Pages document has no TP.DocumentArchive; the container may \
                 be corrupt or password-protected"
                    .into(),
            )
        })?;
    let storage = first_bytes(&document.payload, 4)
        .and_then(reference)
        .and_then(|id| by_id.get(&id))
        .filter(|a| a.ty == TYPE_TEXT_STORAGE)
        .ok_or_else(|| {
            ConversionError::Parse(
                "iwork: the Pages document does not reference a body text storage".into(),
            )
        })?;

    // The body text (field 3, possibly several runs) and its paragraph style
    // run table (field 5).
    let mut text = String::new();
    for (f, v) in Fields::new(&storage.payload) {
        if let (3, Value::Bytes(b)) = (f, v) {
            text.push_str(&String::from_utf8_lossy(b));
        }
    }
    let runs = iwa_style_runs(&storage.payload, &by_id);
    push_pages_paragraphs(split_pages_paragraphs(&text, &runs), doc);

    // Pages keeps tables outside the body text flow, so they cannot be
    // interleaved with the paragraphs and are appended instead.
    for id in &order {
        let Some(model) = by_id.get(id) else {
            continue;
        };
        if model.ty == TYPE_TST_TABLE_MODEL {
            if let Some(table) = iwa_table(model, &by_id) {
                doc.push(Node::Table(table));
            }
        }
    }
    Ok(())
}

/// docling's `_iwa_style_runs`: the storage's paragraph style run table
/// resolved to `(character index, style name)`, in index order. Entries
/// without a style reference leave the previous style in force and are
/// skipped.
fn iwa_style_runs(
    storage_payload: &[u8],
    by_id: &HashMap<u64, &Archive>,
) -> Vec<(usize, Option<String>)> {
    let Some(table) = first_bytes(storage_payload, 5) else {
        return Vec::new();
    };
    let mut runs: Vec<(usize, Option<String>)> = Vec::new();
    for (f, v) in Fields::new(table) {
        let (1, Value::Bytes(entry)) = (f, v) else {
            continue;
        };
        let (Some(index), Some(target)) = (
            first_varint(entry, 1),
            first_bytes(entry, 2).and_then(reference),
        ) else {
            continue;
        };
        let Some(style) = by_id
            .get(&target)
            .filter(|a| a.ty == TYPE_TSWP_PARAGRAPH_STYLE)
        else {
            continue;
        };
        runs.push((index as usize, iwa_style_name(&style.payload)));
    }
    runs.sort_by_key(|run| run.0);
    runs
}

/// docling's `_iwa_style_name`: the style's name out of its `TSS.StyleArchive`
/// super message; `None` for an anonymous style (or a non-UTF-8 name).
fn iwa_style_name(payload: &[u8]) -> Option<String> {
    let super_message = first_bytes(payload, 1)?;
    let name = first_bytes(super_message, 1)?;
    std::str::from_utf8(name).ok().map(str::to_string)
}

/// docling's `_split_paragraphs`: Apple separates paragraphs with newlines;
/// the style runs are keyed by character index into the text and each stays
/// in force until the next begins. Blank paragraphs are dropped.
fn split_pages_paragraphs(
    text: &str,
    style_runs: &[(usize, Option<String>)],
) -> Vec<(String, PagesLabel)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    let mut run_index = 0usize;
    let mut current: Option<&str> = None;
    for line in text.split('\n') {
        while run_index < style_runs.len() && style_runs[run_index].0 <= offset {
            current = style_runs[run_index].1.as_deref();
            run_index += 1;
        }
        let cleaned = clean_pages_text(line);
        if !cleaned.is_empty() {
            out.push((cleaned, label_for_style(current)));
        }
        // Python string indices count code points; + 1 for the newline.
        offset += line.chars().count() + 1;
    }
    out
}

/// One `TST.TableModelArchive` as a grid (docling's `_iwa_tables`): geometry
/// from the model (field 6 rows, 7 columns, 9 header rows), cell contents
/// from the shared string list behind the data store (field 4), their
/// placement from the store's tiles. Only text cells are read — a number, a
/// date or a formula result is left empty rather than guessed at. `None` when
/// nothing readable is in the table.
fn iwa_table(model: &Archive, by_id: &HashMap<u64, &Archive>) -> Option<Table> {
    let num_rows = first_varint(&model.payload, 6)? as usize;
    let num_cols = first_varint(&model.payload, 7)? as usize;
    let store = first_bytes(&model.payload, 4)?;
    if num_rows == 0 || num_cols == 0 {
        return None;
    }
    let header_rows = first_varint(&model.payload, 9).unwrap_or(0) as usize;
    let strings = iwa_string_table(store, by_id);
    let mut cells: Vec<TableCell> = Vec::new();
    for tile in iwa_tiles(store, by_id) {
        iwa_tile_cells(tile, &strings, num_cols, header_rows, &mut cells);
    }
    if cells.is_empty() {
        return None;
    }
    Some(grid_table(num_rows, num_cols, cells))
}

/// A `Table` from first-class cells: the dense `rows` grid every serializer
/// renders (unread positions empty), plus the cells themselves so JSON
/// carries docling's per-cell `column_header` flags (`row < header_rows`).
fn grid_table(num_rows: usize, num_cols: usize, cells: Vec<TableCell>) -> Table {
    let mut rows = vec![vec![String::new(); num_cols]; num_rows];
    for cell in &cells {
        if cell.start_row < num_rows && cell.start_col < num_cols {
            rows[cell.start_row][cell.start_col] = cell.text.clone();
        }
    }
    Table {
        rows,
        cells: Some(cells),
        ..Default::default()
    }
}

fn text_cell(text: String, row: usize, col: usize, header_rows: usize) -> TableCell {
    TableCell {
        text,
        bbox: None,
        start_row: row,
        start_col: col,
        row_span: 1,
        col_span: 1,
        column_header: row < header_rows,
        row_header: false,
        row_section: false,
    }
}

/// docling's `_iwa_string_table`: the table's shared strings (`TST.TableDataList`
/// behind store field 4), keyed as its cells reference them.
fn iwa_string_table(store: &[u8], by_id: &HashMap<u64, &Archive>) -> HashMap<u64, String> {
    let mut strings = HashMap::new();
    let Some(list) = first_bytes(store, 4)
        .and_then(reference)
        .and_then(|id| by_id.get(&id))
        .filter(|a| a.ty == TYPE_TST_DATA_LIST)
    else {
        return strings;
    };
    for (f, v) in Fields::new(&list.payload) {
        let (3, Value::Bytes(entry)) = (f, v) else {
            continue;
        };
        if let (Some(key), Some(value)) = (first_varint(entry, 1), first_bytes(entry, 3)) {
            strings.insert(key, String::from_utf8_lossy(value).into_owned());
        }
    }
    strings
}

/// docling's `_iwa_tiles`: the `TST.Tile`s the data store (field 3) points at.
fn iwa_tiles<'a>(store: &[u8], by_id: &HashMap<u64, &'a Archive>) -> Vec<&'a Archive> {
    let Some(container) = first_bytes(store, 3) else {
        return Vec::new();
    };
    Fields::new(container)
        .filter_map(|(f, v)| match (f, v) {
            (1, Value::Bytes(entry)) => first_bytes(entry, 2)
                .and_then(reference)
                .and_then(|id| by_id.get(&id).copied())
                .filter(|a| a.ty == TYPE_TST_TILE),
            _ => None,
        })
        .collect()
}

/// docling's `_iwa_tile_cells`: each tile row (field 5) holds a packed cell
/// buffer (field 3) plus one `int16` offset per column (field 4), a negative
/// offset marking a column with no cell.
fn iwa_tile_cells(
    tile: &Archive,
    strings: &HashMap<u64, String>,
    num_cols: usize,
    header_rows: usize,
    out: &mut Vec<TableCell>,
) {
    for (f, v) in Fields::new(&tile.payload) {
        let (5, Value::Bytes(row)) = (f, v) else {
            continue;
        };
        let (Some(row_index), Some(storage), Some(offsets)) = (
            first_varint(row, 1),
            first_bytes(row, 3),
            first_bytes(row, 4),
        ) else {
            continue;
        };
        let row_index = row_index as usize;
        for column in 0..num_cols.min(offsets.len() / 2) {
            let start = i16::from_le_bytes([offsets[2 * column], offsets[2 * column + 1]]);
            let Some(text) = iwa_cell_text(storage, start, strings) else {
                continue;
            };
            out.push(text_cell(text, row_index, column, header_rows));
        }
    }
}

/// docling's `_iwa_cell_text`: one packed cell — byte 0 the storage version
/// (4), byte 1 the value type (3 = text), the string key in the four bytes at
/// offset 16. Anything else yields no text rather than misread bytes.
fn iwa_cell_text(storage: &[u8], start: i16, strings: &HashMap<u64, String>) -> Option<String> {
    if start < 0 {
        return None;
    }
    let start = start as usize;
    let key_at = start.checked_add(16)?;
    let key_bytes = storage.get(key_at..key_at + 4)?;
    if storage[start] != 4 || storage[start + 1] != 3 {
        return None;
    }
    let key = u64::from(u32::from_le_bytes(key_bytes.try_into().ok()?));
    strings.get(&key).cloned()
}

/// iWork '09 and earlier: docling's `_read_legacy_document` over `index.xml`
/// (or `index.xml.gz`, inflated against a ceiling the stored size cannot
/// vouch for).
fn convert_pages_legacy(
    pkg: &mut Package,
    member: &str,
    doc: &mut DoclingDocument,
) -> Result<(), ConversionError> {
    let raw = pkg.read_bytes(member).ok_or_else(|| {
        ConversionError::Parse(format!(
            "iwork: could not read '{member}' from the Pages document"
        ))
    })?;
    let raw = if member.ends_with(".gz") {
        gunzip_capped(&raw, MAX_LEGACY_XML_BYTES, member)?
    } else {
        raw
    };
    let xml = String::from_utf8(raw)
        .map_err(|_| ConversionError::Parse(format!("iwork: '{member}' is not UTF-8")))?;
    let dom = roxmltree::Document::parse(&xml)
        .map_err(|e| ConversionError::Parse(format!("iwork: could not parse '{member}': {e}")))?;
    push_pages_paragraphs(legacy_paragraphs(&dom), doc);
    for table in legacy_tables(&dom) {
        doc.push(Node::Table(table));
    }
    Ok(())
}

fn gunzip_capped(raw: &[u8], cap: u64, member: &str) -> Result<Vec<u8>, ConversionError> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(raw)
        .take(cap + 1)
        .read_to_end(&mut out)
        .map_err(|_| ConversionError::Parse(format!("iwork: could not decompress '{member}'")))?;
    if out.len() as u64 > cap {
        return Err(ConversionError::Parse(format!(
            "iwork: '{member}' expands beyond the {cap} byte limit"
        )));
    }
    Ok(out)
}

fn is_sf(node: roxmltree::Node, name: &str) -> bool {
    node.is_element()
        && node.tag_name().namespace() == Some(SF_NS)
        && node.tag_name().name() == name
}

/// docling's `_iter_body_paragraphs` + `_iter_text_excluding_ghosts`: the
/// body `sf:p` paragraphs in document order, pruning page furniture
/// (`sf:header`, `sf:footer`, `sf:footnotes` each hold their own text body,
/// which the IWA reader never sees either) and skipping `sf:ghost-text` —
/// the template placeholder shown before the author types anything. Styles
/// resolve through `sf:paragraphstyle` ident → name.
fn legacy_paragraphs(dom: &roxmltree::Document) -> Vec<(String, PagesLabel)> {
    let mut style_names: HashMap<&str, Option<&str>> = HashMap::new();
    for style in dom.descendants().filter(|n| is_sf(*n, "paragraphstyle")) {
        if let Some(ident) = style.attribute((SF_NS, "ident")) {
            style_names.insert(ident, style.attribute((SF_NS, "name")));
        }
    }

    let mut paragraphs = Vec::new();
    let mut stack = vec![dom.root_element()];
    while let Some(node) = stack.pop() {
        if is_sf(node, "p") {
            paragraphs.push(node);
        }
        // Reverse so children pop in document order.
        for child in node.children().rev() {
            if child.is_element()
                && !(is_sf(child, "header") || is_sf(child, "footer") || is_sf(child, "footnotes"))
            {
                stack.push(child);
            }
        }
    }

    let mut out = Vec::new();
    for para in paragraphs {
        let mut text = String::new();
        for node in para.descendants().filter(|n| n.is_text()) {
            let in_ghost = node
                .ancestors()
                .take_while(|a| *a != para)
                .any(|a| is_sf(a, "ghost-text"));
            if !in_ghost {
                text.push_str(node.text().unwrap_or(""));
            }
        }
        let cleaned = clean_pages_text(&text);
        if cleaned.is_empty() {
            continue;
        }
        let style = para
            .attribute((SF_NS, "style"))
            .and_then(|ident| style_names.get(ident).copied())
            .flatten();
        out.push((cleaned, label_for_style(style)));
    }
    out
}

fn int_attr(node: roxmltree::Node, name: &str) -> Option<usize> {
    node.attribute((SF_NS, name))?.trim().parse().ok()
}

/// docling's `_read_legacy_tables`: cells are stored flat in row-major order,
/// so the `sf:grid` dimensions give them their positions; `sf:num-header-rows`
/// on the model marks the header rows.
fn legacy_tables(dom: &roxmltree::Document) -> Vec<Table> {
    let mut tables = Vec::new();
    for model in dom.descendants().filter(|n| is_sf(*n, "tabular-model")) {
        let Some(grid) = model.descendants().find(|n| is_sf(*n, "grid")) else {
            continue;
        };
        let (Some(num_cols), Some(num_rows)) =
            (int_attr(grid, "numcols"), int_attr(grid, "numrows"))
        else {
            continue;
        };
        if num_cols == 0 || num_rows == 0 {
            continue;
        }
        let header_rows = int_attr(model, "num-header-rows").unwrap_or(0);
        let values: Vec<String> = model
            .descendants()
            .filter(|n| is_sf(*n, "ct"))
            .map(|cell| {
                let text = match cell.attribute((SFA_NS, "s")) {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => cell
                        .descendants()
                        .filter_map(|n| n.text())
                        .collect::<String>(),
                };
                clean_pages_text(&text)
            })
            .collect();
        if values.is_empty() {
            continue;
        }
        let cells = values
            .into_iter()
            .take(num_cols * num_rows)
            .enumerate()
            .map(|(index, text)| text_cell(text, index / num_cols, index % num_cols, header_rows))
            .collect();
        tables.push(grid_table(num_rows, num_cols, cells));
    }
    tables
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

    /// docling's `_label_for_style` table (its test_style_names_map_to_labels).
    #[test]
    fn style_names_map_to_labels_like_docling() {
        use PagesLabel::*;
        for (name, want) in [
            (Some("Title"), Title),
            (Some("Heading 1"), Heading(1)),
            (Some("Heading 2"), Heading(2)),
            (Some("Heading"), Heading(1)),
            (Some("heading 9"), Heading(6)),
            (Some("Subheading"), Heading(2)),
            (Some("Subtitle"), Heading(2)),
            (Some("Body"), Text),
            (Some("Free Form"), Text),
            (Some("Footnote Text"), Text),
            (Some("Heading one"), Text),
            (None, Text),
        ] {
            assert_eq!(label_for_style(name), want, "{name:?}");
        }
    }

    /// docling's `_split_paragraphs`: runs keyed by code-point index, blanks
    /// dropped, U+FFFC attachments removed.
    #[test]
    fn paragraph_split_follows_style_runs() {
        let runs = vec![
            (0, Some("Title".to_string())),
            (6, Some("Body".to_string())),
        ];
        let paras = split_pages_paragraphs("Titl\u{FFFC}e\n\nBödy one\nBody two", &runs);
        assert_eq!(
            paras,
            vec![
                ("Title".to_string(), PagesLabel::Title),
                ("Bödy one".to_string(), PagesLabel::Text),
                ("Body two".to_string(), PagesLabel::Text),
            ]
        );
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
