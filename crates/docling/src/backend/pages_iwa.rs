//! Reader for the IWA object graph of a Pages 5+ document (docling's
//! `docling/backend/iwork/pages_iwa.py`, docling#4062, #383).
//!
//! Apple publishes no schema; the message and field numbers below are
//! upstream's, established against real documents. Only the message *type
//! numbers* are format knowledge — every field is read positionally through
//! the generic wire walk in [`super::iwork`].

use std::collections::{HashMap, HashSet};

use docling_core::{Table, TableCell};

use super::iwork::{
    decode_iwa, first_bytes, first_varint, grid_table, parse_archives, reference, text_cell,
    Archive, Fields, Value,
};
use super::ooxml::Package;
use super::pages::{
    authored, runs_for, script_of, split_paragraphs, unique_paragraphs, value_at, Block, Comment,
    Content, Formatting, ListStyle, Paragraph, Picture, StorageRuns,
};
use crate::error::ConversionError;

/// How deep `referenced_ids` follows nested messages.
const MAX_REFERENCE_DEPTH: usize = 4;
/// A `TSP.Reference` never encodes to more than this; longer bytes are nested
/// messages to descend into.
const REFERENCE_MAX_BYTES: usize = 6;

/// `TSWP.CharacterStyleArchive`.
const TSWP_CHARACTER_STYLE: u32 = 2021;
/// Storage field holding the character-style run table.
const STORAGE_CHARACTER_STYLE_FIELD: u32 = 8;
/// A style's property map.
const STYLE_PROPERTIES_FIELD: u32 = 11;
/// The property holding a style's superscript/subscript setting.
const STYLE_SCRIPT_FIELD: u32 = 10;
/// Property fields of a character style as they map onto formatting —
/// established upstream by correlating style names with their properties:
/// "Emphasis"/"Bold" set field 1, "Italic" field 2, "Underline" and "Link"
/// field 11, "Strikethrough" field 12.
const PROPERTY_BOLD: u32 = 1;
const PROPERTY_ITALIC: u32 = 2;
const PROPERTY_UNDERLINE: u32 = 11;
const PROPERTY_STRIKETHROUGH: u32 = 12;

/// `TSWP.ShapeInfoArchive` — a text box; its storages hold the text.
const TSWP_SHAPE_INFO: u32 = 2011;
/// `TP.DocumentArchive` field naming the container of the floating drawables.
const DOCUMENT_DRAWABLES_FIELD: u32 = 20;
/// `TP.DocumentArchive` — the root object of a Pages document.
const TP_DOCUMENT_ARCHIVE: u32 = 10000;
/// `TSWP.StorageArchive` — every piece of rich text in any iWork app.
const TSWP_STORAGE_ARCHIVE: u32 = 2001;
/// `TSWP.ParagraphStyleArchive` — a paragraph style whose `TSS.StyleArchive`
/// super (field 1) carries the human-facing name ("Body", "Heading 1").
const TSWP_PARAGRAPH_STYLE: u32 = 2022;
/// `TP.DocumentArchive` field referencing the body text storage.
const DOCUMENT_BODY_FIELD: u32 = 4;
/// Storage field holding the text pieces.
const STORAGE_TEXT_FIELD: u32 = 3;
const STYLE_SUPER_FIELD: u32 = 1;
const STYLE_NAME_FIELD: u32 = 1;
/// `TST.TableModelArchive` — table name + data store.
const TST_TABLE_MODEL: u32 = 6001;
/// `TST.TableInfoArchive` — a table drawable, references the model.
const TST_TABULAR_INFO: u32 = 6000;
const TABULAR_INFO_MODEL_FIELD: u32 = 2;
/// `TSD.ImageArchive`.
const TSD_IMAGE: u32 = 3005;
/// `TSD.GroupArchive` — drawables grouped together.
const TSD_GROUP: u32 = 3008;
const GROUP_CHILDREN_FIELD: u32 = 2;
/// The fields of an image that may name its data, in the order upstream tries
/// them.
const IMAGE_DATA_FIELDS: [u32; 4] = [15, 13, 11, 12];
/// `TSWP.DrawableAttachmentArchive` — anchors a drawable in the text.
const TSWP_DRAWABLE_ATTACHMENT: u32 = 2003;
const ATTACHMENT_DRAWABLE_FIELD: u32 = 1;
/// Storage field holding the attachment run table.
const STORAGE_ATTACHMENT_FIELD: u32 = 9;
/// Storage field holding the comment run table.
const STORAGE_COMMENT_FIELD: u32 = 23;
/// `TSWP.CommentFieldArchive` — a comment's highlight over the text.
const TSWP_COMMENT_FIELD: u32 = 2013;
const COMMENT_FIELD_STORAGE_FIELD: u32 = 1;
/// `TSD.CommentStorageArchive` — one comment (or reply).
const TSD_COMMENT_STORAGE: u32 = 3056;
const COMMENT_TEXT_FIELD: u32 = 1;
const COMMENT_AUTHOR_FIELD: u32 = 3;
const COMMENT_REPLIES_FIELD: u32 = 4;
/// `TSK.AnnotationAuthorArchive`.
const TSK_ANNOTATION_AUTHOR: u32 = 212;
const AUTHOR_NAME_FIELD: u32 = 1;
/// `TSWP.FootnoteReferenceAttachmentArchive` — a footnote, holding its text
/// storage.
const TSWP_NOTE: u32 = 2008;
const NOTE_STORAGE_FIELD: u32 = 2;
/// Storage field holding the footnote run table.
const STORAGE_FOOTNOTE_FIELD: u32 = 16;
/// Storage field holding the page-master run table.
const STORAGE_PAGE_MASTER_FIELD: u32 = 17;
/// `TP.PageMasterArchive`.
const TP_PAGE_MASTER: u32 = 10011;
/// The page master's first-page, even-page and odd-page header/footer bundles.
const PAGE_MASTER_HEADER_FOOTER_FIELDS: [u32; 3] = [23, 24, 25];
/// `TP.HeadersAndFootersArchive`.
const TP_HEADERS_AND_FOOTERS: u32 = 10143;
const HEADERS_FIELD: u32 = 1;
const FOOTERS_FIELD: u32 = 2;
/// `TSP.PackageMetadata` — names the container member behind every data id.
const TSP_PACKAGE_METADATA: u32 = 11006;
const PACKAGE_DATAS_FIELD: u32 = 4;
const DATA_INFO_IDENTIFIER_FIELD: u32 = 1;
const DATA_INFO_PREFERRED_NAME_FIELD: u32 = 3;
const DATA_INFO_NAME_FIELD: u32 = 4;
const DATA_MEMBER_PREFIX: &str = "Data/";
/// `TST.Tile` — lays a table's cells out into rows.
const TST_TILE: u32 = 6002;
/// `TST.TableDataList` — shared per-table value lists.
const TST_DATA_LIST: u32 = 6005;
const TABLE_ROWS_FIELD: u32 = 6;
const TABLE_COLS_FIELD: u32 = 7;
const TABLE_HEADER_ROWS_FIELD: u32 = 9;
const TABLE_DATA_STORE_FIELD: u32 = 4;
const STORE_TILES_FIELD: u32 = 3;
const STORE_STRINGS_FIELD: u32 = 4;
const STORE_RICH_TEXT_FIELD: u32 = 17;
const LIST_ENTRIES_FIELD: u32 = 3;
/// A data list that spills into further segments references them here.
const LIST_SEGMENTS_FIELD: u32 = 4;
const ENTRY_KEY_FIELD: u32 = 1;
const ENTRY_STRING_FIELD: u32 = 3;
const ENTRY_RICH_TEXT_FIELD: u32 = 9;
/// `TST.RichTextPayloadArchive` — indirection to a cell's text storage.
const TST_TEXT_REF: u32 = 6218;
const TILE_ROWS_FIELD: u32 = 5;
const ROW_INDEX_FIELD: u32 = 1;
const ROW_STORAGE_FIELD: u32 = 3;
const ROW_OFFSETS_FIELD: u32 = 4;
const ROW_WIDE_STORAGE_FIELD: u32 = 6;
const ROW_WIDE_OFFSETS_FIELD: u32 = 7;
const ROW_WIDE_OFFSETS_FLAG: u32 = 8;
/// Storage versions of a packed cell, in byte 0.
const CELL_VERSION_LEGACY: u8 = 4;
const CELL_VERSION_CURRENT: u8 = 5;
/// Value types of a packed cell, in byte 1, that carry text.
const CELL_TYPE_TEXT: u8 = 3;
const CELL_TYPE_RICH_TEXT: u8 = 9;
/// Where a version 4 cell keeps the key of its string.
const CELL_KEY_OFFSET: usize = 16;
/// Where a version 5 cell keeps its flags, and where its values begin. The
/// flags say which values are present; each one that is takes a fixed width,
/// so the position of any of them depends on all the ones before it.
const CELL_FLAGS_OFFSET: usize = 8;
const CELL_VALUES_OFFSET: usize = 12;
const CELL_FLAG_STRING: u32 = 0x8;
const CELL_FLAG_RICH_TEXT: u32 = 0x10;
/// The values a version 5 cell may hold, in the order they are laid out: a
/// decimal, a double and a duration first, then the keys of the string and
/// the rich text a cell may reference. Nothing after the rich text key is
/// needed, so the walk stops there.
const CELL_VALUE_WIDTHS: [(u32, usize); 5] = [
    (0x1, 16),
    (0x2, 8),
    (0x4, 8),
    (CELL_FLAG_STRING, 4),
    (CELL_FLAG_RICH_TEXT, 4),
];
/// Storage field holding the paragraph-style run table.
const STORAGE_PARAGRAPH_STYLE_FIELD: u32 = 5;
/// `TSWP.HyperlinkFieldArchive`.
const TSWP_LINK_FIELD: u32 = 2032;
const LINK_URL_FIELD: u32 = 2;
/// Storage field holding the smart-field (hyperlink) run table.
const STORAGE_SMART_FIELD: u32 = 11;
/// `TSWP.ListStyleArchive`.
const TSWP_LIST_STYLE: u32 = 2023;
/// Storage field holding the list-depth run table (numbers, not references).
const STORAGE_LIST_DEPTH_FIELD: u32 = 6;
/// Storage field holding the list-style run table.
const STORAGE_LIST_STYLE_FIELD: u32 = 7;
/// A list style's per-depth label types and marker strings.
const LIST_LABEL_TYPES_FIELD: u32 = 11;
const LIST_STRINGS_FIELD: u32 = 16;

type Objects<'a> = HashMap<u64, &'a Archive>;

fn fail(msg: &str) -> ConversionError {
    ConversionError::Parse(format!("iwork: {msg}"))
}

/// docling's `read_content`: the content of a Pages 5+ document out of its
/// IWA object graph. Objects are keyed by identifier over every `.iwa` member
/// in archive order, every message included, later definitions replacing
/// earlier ones (a Python dict keeps the first key's position with the last
/// value) — reproduced so the body lookup and the drawable order match
/// upstream exactly.
pub(crate) fn read_content(pkg: &mut Package) -> Result<Content, ConversionError> {
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
    let mut objects: Objects = HashMap::new();
    for a in &archives {
        if objects.insert(a.id, a).is_none() {
            order.push(a.id);
        }
    }

    let document = order
        .iter()
        .filter_map(|id| objects.get(id).copied())
        .find(|a| a.ty == TP_DOCUMENT_ARCHIVE)
        .ok_or_else(|| {
            fail(
                "the Pages document has no TP.DocumentArchive; the container may be corrupt \
                 or password-protected",
            )
        })?;
    let storage = first_bytes(&document.payload, DOCUMENT_BODY_FIELD)
        .and_then(reference)
        .and_then(|id| objects.get(&id).copied())
        .filter(|a| a.ty == TSWP_STORAGE_ARCHIVE)
        .ok_or_else(|| fail("the Pages document does not reference a body text storage"))?;

    let mut reader = Reader {
        objects: &objects,
        order: &order,
        pkg,
        data_files: HashMap::new(),
        emitted: HashSet::new(),
    };
    reader.data_files = reader.data_files();
    let mut blocks = reader.storage_blocks(storage);
    blocks.extend(reader.floating_blocks(document));
    let (headers, footers) = reader.page_furniture(storage);
    Ok(Content {
        blocks,
        headers,
        footers,
        footnotes: reader.footnotes(storage),
        comments: reader.comments(storage),
    })
}

/// `iwa_style_name`: a paragraph style's name out of its `TSS` super message;
/// `None` for an anonymous (ad-hoc formatting) style.
fn style_name(payload: &[u8]) -> Option<String> {
    let super_message = first_bytes(payload, STYLE_SUPER_FIELD)?;
    let name = first_bytes(super_message, STYLE_NAME_FIELD)?;
    std::str::from_utf8(name).ok().map(str::to_string)
}

/// `iwa_storage_text`: the text pieces of a storage joined, as code points.
fn storage_text(payload: &[u8]) -> Vec<char> {
    let mut text = String::new();
    for (f, v) in Fields::new(payload) {
        if let (STORAGE_TEXT_FIELD, Value::Bytes(b)) = (f, v) {
            text.push_str(&String::from_utf8_lossy(b));
        }
    }
    text.chars().collect()
}

/// `iwa_object_runs`: one `TSWP.ObjectAttributeTable` resolved to (character
/// index, value) pairs. An entry without a reference (or one whose target is
/// not of `message_type`) clears the value from that character on, which is
/// how Pages ends a bold phrase or leaves a list.
fn object_runs<T>(
    payload: &[u8],
    field: u32,
    objects: &Objects,
    message_type: u32,
    decode: impl Fn(&[u8]) -> Option<T>,
) -> Vec<(usize, Option<T>)> {
    let Some(table) = first_bytes(payload, field) else {
        return Vec::new();
    };
    let mut runs: Vec<(usize, Option<T>)> = Vec::new();
    for (f, v) in Fields::new(table) {
        let (1, Value::Bytes(entry)) = (f, v) else {
            continue;
        };
        let Some(index) = first_varint(entry, 1) else {
            continue;
        };
        let value = first_bytes(entry, 2)
            .and_then(reference)
            .and_then(|id| objects.get(&id))
            .filter(|a| a.ty == message_type)
            .and_then(|a| decode(&a.payload));
        runs.push((index as usize, value));
    }
    runs.sort_by_key(|run| run.0);
    runs
}

/// `iwa_depth_runs`: the list depth table, whose entries hold numbers.
fn depth_runs(payload: &[u8]) -> Vec<(usize, usize)> {
    let Some(table) = first_bytes(payload, STORAGE_LIST_DEPTH_FIELD) else {
        return Vec::new();
    };
    let mut runs = Vec::new();
    for (f, v) in Fields::new(table) {
        let (1, Value::Bytes(entry)) = (f, v) else {
            continue;
        };
        if let (Some(index), Some(depth)) = (first_varint(entry, 1), first_varint(entry, 2)) {
            runs.push((index as usize, depth as usize));
        }
    }
    runs.sort_by_key(|run| run.0);
    runs
}

/// `iwa_attachment_runs`: an anchoring run table resolved to (character
/// index, object id) pairs. Unlike the style tables, an entry anchors an
/// object at one character rather than putting a value in force from it, so
/// entries without a reference carry nothing and are dropped.
fn attachment_runs(payload: &[u8], field: u32) -> Vec<(usize, u64)> {
    let Some(table) = first_bytes(payload, field) else {
        return Vec::new();
    };
    let mut runs = Vec::new();
    for (f, v) in Fields::new(table) {
        let (1, Value::Bytes(entry)) = (f, v) else {
            continue;
        };
        if let (Some(index), Some(target)) = (
            first_varint(entry, 1),
            first_bytes(entry, 2).and_then(reference),
        ) {
            runs.push((index as usize, target));
        }
    }
    runs.sort_by_key(|run| run.0);
    runs
}

/// `iwa_link`: the address a hyperlink field points at.
fn link(payload: &[u8]) -> Option<String> {
    let url = first_bytes(payload, LINK_URL_FIELD)?;
    let url = String::from_utf8_lossy(url).trim().to_string();
    (!url.is_empty()).then_some(url)
}

/// `iwa_list_style`: a list style as its per-depth label ladder.
fn list_style(payload: &[u8]) -> Option<ListStyle> {
    let mut style = ListStyle::default();
    for (f, v) in Fields::new(payload) {
        match (f, v) {
            (LIST_LABEL_TYPES_FIELD, Value::Varint(n)) => style.label_types.push(n),
            (LIST_STRINGS_FIELD, Value::Bytes(b)) => {
                style.strings.push(String::from_utf8_lossy(b).into_owned())
            }
            _ => {}
        }
    }
    Some(style)
}

/// `iwa_formatting`: a character style's property map as formatting.
fn formatting(payload: &[u8]) -> Option<Formatting> {
    let properties = first_bytes(payload, STYLE_PROPERTIES_FIELD)?;
    let (mut bold, mut italic, mut underline, mut strike) = (false, false, false, false);
    let mut script = None;
    for (f, v) in Fields::new(properties) {
        let Value::Varint(n) = v else {
            continue;
        };
        match f {
            PROPERTY_BOLD if n != 0 => bold = true,
            PROPERTY_ITALIC if n != 0 => italic = true,
            PROPERTY_UNDERLINE if n != 0 => underline = true,
            PROPERTY_STRIKETHROUGH if n != 0 => strike = true,
            STYLE_SCRIPT_FIELD if script.is_none() => script = Some(n),
            _ => {}
        }
    }
    Formatting::build(bold, italic, underline, strike, script.and_then(script_of))
}

/// `iwa_storage_runs`: every run table a storage carries.
fn storage_runs(payload: &[u8], objects: &Objects) -> StorageRuns {
    StorageRuns {
        styles: object_runs(
            payload,
            STORAGE_PARAGRAPH_STYLE_FIELD,
            objects,
            TSWP_PARAGRAPH_STYLE,
            style_name,
        ),
        characters: object_runs(
            payload,
            STORAGE_CHARACTER_STYLE_FIELD,
            objects,
            TSWP_CHARACTER_STYLE,
            formatting,
        ),
        lists: object_runs(
            payload,
            STORAGE_LIST_STYLE_FIELD,
            objects,
            TSWP_LIST_STYLE,
            list_style,
        ),
        depths: depth_runs(payload),
        links: object_runs(payload, STORAGE_SMART_FIELD, objects, TSWP_LINK_FIELD, link),
    }
}

// --- tables -----------------------------------------------------------------

/// The plain strings and the rich text of a table, each keyed by cell
/// reference.
struct CellValues {
    strings: HashMap<u32, String>,
    rich_text: HashMap<u32, String>,
}

/// `iwa_table`: a table keeps its geometry on the model, its cell contents in
/// a shared value list, and the placement of those values in tiles. Cells
/// reference their value by key, so equal values share one entry — which is
/// why the tiles have to be read rather than assuming the value list is
/// already in cell order.
fn table(model: &Archive, objects: &Objects) -> Option<Table> {
    let num_rows = first_varint(&model.payload, TABLE_ROWS_FIELD)? as usize;
    let num_cols = first_varint(&model.payload, TABLE_COLS_FIELD)? as usize;
    let store = first_bytes(&model.payload, TABLE_DATA_STORE_FIELD)?;
    if num_rows == 0 || num_cols == 0 {
        return None;
    }
    let header_rows = first_varint(&model.payload, TABLE_HEADER_ROWS_FIELD).unwrap_or(0) as usize;
    let values = CellValues {
        strings: value_list(store, STORE_STRINGS_FIELD, objects, entry_string),
        rich_text: value_list(store, STORE_RICH_TEXT_FIELD, objects, entry_rich_text),
    };
    let mut cells: Vec<TableCell> = Vec::new();
    for tile in tiles(store, objects) {
        tile_cells(tile, &values, num_cols, header_rows, &mut cells);
    }
    if cells.is_empty() {
        return None;
    }
    Some(grid_table(num_rows, num_cols, cells))
}

/// `iwa_value_list`: one `TST.TableDataList`, following any segments it spills
/// into.
fn value_list(
    store: &[u8],
    field: u32,
    objects: &Objects,
    decode: impl Fn(&[u8], &Objects) -> Option<String>,
) -> HashMap<u32, String> {
    let mut values = HashMap::new();
    let Some(list) = first_bytes(store, field)
        .and_then(reference)
        .and_then(|id| objects.get(&id))
        .filter(|a| a.ty == TST_DATA_LIST)
    else {
        return values;
    };
    let mut payloads: Vec<&[u8]> = vec![&list.payload];
    for segment in reference_list(&list.payload, LIST_SEGMENTS_FIELD) {
        if let Some(spilled) = objects.get(&segment) {
            payloads.push(&spilled.payload);
        }
    }
    for payload in payloads {
        for (f, v) in Fields::new(payload) {
            let (LIST_ENTRIES_FIELD, Value::Bytes(entry)) = (f, v) else {
                continue;
            };
            if let (Some(key), Some(value)) =
                (first_varint(entry, ENTRY_KEY_FIELD), decode(entry, objects))
            {
                values.insert(key as u32, value);
            }
        }
    }
    values
}

/// `iwa_entry_string`: a value list entry that holds its string directly.
fn entry_string(entry: &[u8], _objects: &Objects) -> Option<String> {
    first_bytes(entry, ENTRY_STRING_FIELD).map(|b| String::from_utf8_lossy(b).into_owned())
}

/// `iwa_entry_rich_text`: a value list entry that points at a whole text
/// storage.
fn entry_rich_text(entry: &[u8], objects: &Objects) -> Option<String> {
    let indirection = first_bytes(entry, ENTRY_RICH_TEXT_FIELD)
        .and_then(reference)
        .and_then(|id| objects.get(&id))
        .filter(|a| a.ty == TST_TEXT_REF)?;
    let storage = reference_field(&indirection.payload, 1)
        .and_then(|id| objects.get(&id))
        .filter(|a| a.ty == TSWP_STORAGE_ARCHIVE)?;
    let text: String = storage_text(&storage.payload).into_iter().collect();
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// `iwa_tiles`: the tiles a table's data store points at.
fn tiles<'a>(store: &[u8], objects: &Objects<'a>) -> Vec<&'a Archive> {
    let Some(container) = first_bytes(store, STORE_TILES_FIELD) else {
        return Vec::new();
    };
    Fields::new(container)
        .filter_map(|(f, v)| match (f, v) {
            (1, Value::Bytes(entry)) => first_bytes(entry, 2)
                .and_then(reference)
                .and_then(|id| objects.get(&id).copied())
                .filter(|a| a.ty == TST_TILE),
            _ => None,
        })
        .collect()
}

/// `iwa_tile_cells`: one tile's cells, placed by each row's per-column offsets.
fn tile_cells(
    tile: &Archive,
    values: &CellValues,
    num_cols: usize,
    header_rows: usize,
    out: &mut Vec<TableCell>,
) {
    for (f, v) in Fields::new(&tile.payload) {
        let (TILE_ROWS_FIELD, Value::Bytes(row)) = (f, v) else {
            continue;
        };
        let Some(row_index) = first_varint(row, ROW_INDEX_FIELD) else {
            continue;
        };
        let wide = (
            first_bytes(row, ROW_WIDE_STORAGE_FIELD),
            first_bytes(row, ROW_WIDE_OFFSETS_FIELD),
        );
        let (storage, offsets, scale) = match wide {
            (Some(s), Some(o)) => {
                let scale = if first_varint(row, ROW_WIDE_OFFSETS_FLAG).unwrap_or(0) != 0 {
                    4
                } else {
                    1
                };
                (s, o, scale)
            }
            _ => match (
                first_bytes(row, ROW_STORAGE_FIELD),
                first_bytes(row, ROW_OFFSETS_FIELD),
            ) {
                (Some(s), Some(o)) => (s, o, 1),
                _ => continue,
            },
        };
        let row_index = row_index as usize;
        for column in 0..num_cols.min(offsets.len() / 2) {
            let start = i16::from_le_bytes([offsets[2 * column], offsets[2 * column + 1]]);
            let Some(text) = cell_text(storage, i64::from(start) * scale, values) else {
                continue;
            };
            out.push(text_cell(text, row_index, column, header_rows));
        }
    }
}

/// `iwa_cell_text`: one packed cell, or `None` when there is nothing readable
/// there. Only the layouts that carry text are decoded; a number, a date, a
/// formula result is skipped rather than guessed at.
fn cell_text(storage: &[u8], start: i64, values: &CellValues) -> Option<String> {
    if start < 0 {
        return None;
    }
    let start = start as usize;
    if start.checked_add(CELL_VALUES_OFFSET)? > storage.len() {
        return None;
    }
    let version = storage[start];
    if version == CELL_VERSION_LEGACY {
        if storage[start + 1] != CELL_TYPE_TEXT {
            return None;
        }
        let key = read_u32(storage, start + CELL_KEY_OFFSET)?;
        return values.strings.get(&key).cloned();
    }
    if version != CELL_VERSION_CURRENT {
        return None;
    }
    if !matches!(storage[start + 1], CELL_TYPE_TEXT | CELL_TYPE_RICH_TEXT) {
        return None;
    }
    let flags = read_u32(storage, start + CELL_FLAGS_OFFSET)?;
    let mut offset = start + CELL_VALUES_OFFSET;
    for (flag, width) in CELL_VALUE_WIDTHS {
        if flags & flag == 0 {
            continue;
        }
        if offset + width > storage.len() {
            return None;
        }
        if flag == CELL_FLAG_STRING {
            return values.strings.get(&read_u32(storage, offset)?).cloned();
        }
        if flag == CELL_FLAG_RICH_TEXT {
            return values.rich_text.get(&read_u32(storage, offset)?).cloned();
        }
        offset += width;
    }
    None
}

fn read_u32(buffer: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(buffer.get(at..at + 4)?.try_into().ok()?))
}

// --- the object-graph walk --------------------------------------------------

/// `iwa_reference_field`: the object identifier a message's reference field
/// points at.
fn reference_field(payload: &[u8], field: u32) -> Option<u64> {
    first_bytes(payload, field).and_then(reference)
}

/// `iwa_reference_list`: the object identifiers a repeated reference field
/// holds.
fn reference_list(payload: &[u8], field: u32) -> Vec<u64> {
    Fields::new(payload)
        .filter_map(|(f, v)| match v {
            Value::Bytes(b) if f == field => reference(b),
            _ => None,
        })
        .collect()
}

/// `iwa_referenced_ids`: every object identifier a message references, at any
/// nesting up to [`MAX_REFERENCE_DEPTH`].
fn referenced_ids(payload: &[u8], depth: usize, found: &mut HashSet<u64>) {
    if depth > MAX_REFERENCE_DEPTH {
        return;
    }
    for (_, v) in Fields::new(payload) {
        let Value::Bytes(b) = v else {
            continue;
        };
        if b.len() <= REFERENCE_MAX_BYTES {
            if let Some(target) = reference(b) {
                found.insert(target);
            }
        } else {
            referenced_ids(b, depth + 1, found);
        }
    }
}

/// Reads content out of the object graph (upstream's `IWAReader`). Drawables
/// are reached twice over — once from the attachment table of the text they
/// are anchored in, and once from the document's own list of floating ones —
/// so every drawable already emitted is remembered. That also bounds the
/// walk: an object graph may contain cycles.
struct Reader<'a, 'p> {
    objects: &'a Objects<'a>,
    /// Identifiers in first-seen order (Python dict iteration order).
    order: &'a [u64],
    pkg: &'p mut Package,
    /// Data identifier → the `Data/` member holding its bytes.
    data_files: HashMap<u64, String>,
    emitted: HashSet<u64>,
}

impl<'a> Reader<'a, '_> {
    fn object(&self, id: u64) -> Option<&'a Archive> {
        self.objects.get(&id).copied()
    }

    fn typed(&self, id: u64, ty: u32) -> Option<&'a Archive> {
        self.object(id).filter(|a| a.ty == ty)
    }

    /// `storage_blocks`: one storage as paragraphs and anchored drawables.
    /// Apple marks the anchor of a drawable with U+FFFC inside the text, and
    /// the storage's attachment table says which drawable each one is; the
    /// drawable is emitted straight after the paragraph it is anchored in,
    /// which is where it belongs in the reading order.
    fn storage_blocks(&mut self, storage: &Archive) -> Vec<Block> {
        let text = storage_text(&storage.payload);
        let runs = storage_runs(&storage.payload, self.objects);
        let attachments = attachment_runs(&storage.payload, STORAGE_ATTACHMENT_FIELD);
        let comments = attachment_runs(&storage.payload, STORAGE_COMMENT_FIELD);

        let mut blocks = Vec::new();
        let mut offset = 0usize;
        for line in text.split(|&c| c == '\n') {
            let end = offset + line.len() + 1;
            let pieces = runs_for(line, offset, &runs);
            if !pieces.is_empty() {
                let label = super::pages::label_for_style(
                    value_at(&runs.styles, offset).flatten().as_deref(),
                );
                blocks.push(Block::Paragraph(Paragraph {
                    runs: pieces,
                    label,
                    list: super::pages::list_label_at(&runs, offset),
                    anchors: comments
                        .iter()
                        .filter(|(index, _)| offset <= *index && *index < end)
                        .map(|(_, field)| field.to_string())
                        .collect(),
                }));
            }
            for (index, identifier) in &attachments {
                if offset <= *index && *index < end {
                    blocks.extend(self.drawable_blocks(*identifier));
                }
            }
            offset = end;
        }
        blocks
    }

    /// `floating_blocks`: the drawables the document owns rather than anchors
    /// in its text. Reaching them by ownership matters: scanning every storage
    /// in the document would also pick up headers, footers and footnotes.
    fn floating_blocks(&mut self, document: &Archive) -> Vec<Block> {
        let Some(root) = reference_field(&document.payload, DOCUMENT_DRAWABLES_FIELD)
            .and_then(|id| self.object(id))
        else {
            return Vec::new();
        };
        let mut ids = HashSet::new();
        referenced_ids(&root.payload, 0, &mut ids);
        let mut ids: Vec<u64> = ids.into_iter().collect();
        ids.sort_unstable();
        let mut blocks = Vec::new();
        for id in ids {
            blocks.extend(self.drawable_blocks(id));
        }
        blocks
    }

    /// `footnotes`: the notes anchored in one storage, in anchor order.
    fn footnotes(&self, storage: &Archive) -> Vec<Paragraph> {
        let mut paragraphs = Vec::new();
        for (_, identifier) in attachment_runs(&storage.payload, STORAGE_FOOTNOTE_FIELD) {
            let Some(note) = self.typed(identifier, TSWP_NOTE) else {
                continue;
            };
            paragraphs.extend(
                self.storage_paragraphs(reference_field(&note.payload, NOTE_STORAGE_FIELD)),
            );
        }
        paragraphs
    }

    /// `page_furniture`: the headers and footers of the page masters a storage
    /// runs under. Pages writes three sets per master — first page, even pages,
    /// odd pages — whether or not the author filled them in, so identical text
    /// is emitted once.
    fn page_furniture(&self, storage: &Archive) -> (Vec<Paragraph>, Vec<Paragraph>) {
        let mut headers = Vec::new();
        let mut footers = Vec::new();
        for (_, identifier) in attachment_runs(&storage.payload, STORAGE_PAGE_MASTER_FIELD) {
            let Some(master) = self.typed(identifier, TP_PAGE_MASTER) else {
                continue;
            };
            for field in PAGE_MASTER_HEADER_FOOTER_FIELDS {
                let Some(bundle) = reference_field(&master.payload, field)
                    .and_then(|id| self.typed(id, TP_HEADERS_AND_FOOTERS))
                else {
                    continue;
                };
                for text_id in reference_list(&bundle.payload, HEADERS_FIELD) {
                    headers.extend(self.storage_paragraphs(Some(text_id)));
                }
                for text_id in reference_list(&bundle.payload, FOOTERS_FIELD) {
                    footers.extend(self.storage_paragraphs(Some(text_id)));
                }
            }
        }
        (unique_paragraphs(headers), unique_paragraphs(footers))
    }

    /// `comments`: the comments attached to the text of one storage. Pages 5
    /// records a comment as a highlight over the words being commented on; the
    /// run table names a comment field, which holds the comment, and replies
    /// are comments in their own right followed as a chain.
    fn comments(&self, storage: &Archive) -> Vec<Comment> {
        let mut comments = Vec::new();
        for (_, identifier) in attachment_runs(&storage.payload, STORAGE_COMMENT_FIELD) {
            let Some(field) = self.typed(identifier, TSWP_COMMENT_FIELD) else {
                continue;
            };
            let head = reference_field(&field.payload, COMMENT_FIELD_STORAGE_FIELD);
            comments.extend(self.thread(head).into_iter().map(|text| Comment {
                text,
                anchor: identifier.to_string(),
            }));
        }
        comments
    }

    /// `_thread`: one comment and its replies, as text prefixed by their
    /// authors.
    fn thread(&self, head: Option<u64>) -> Vec<String> {
        let mut texts = Vec::new();
        let mut pending: std::collections::VecDeque<u64> = head.into_iter().collect();
        let mut seen = HashSet::new();
        while let Some(current) = pending.pop_front() {
            if !seen.insert(current) {
                continue;
            }
            let Some(comment) = self.typed(current, TSD_COMMENT_STORAGE) else {
                continue;
            };
            if let Some(raw) = first_bytes(&comment.payload, COMMENT_TEXT_FIELD) {
                let text = String::from_utf8_lossy(raw).trim().to_string();
                if !text.is_empty() {
                    texts.push(authored(self.author(&comment.payload).as_deref(), &text));
                }
            }
            pending.extend(reference_list(&comment.payload, COMMENT_REPLIES_FIELD));
        }
        texts
    }

    /// `_author`: the name of whoever wrote a comment.
    fn author(&self, payload: &[u8]) -> Option<String> {
        let author = reference_field(payload, COMMENT_AUTHOR_FIELD)
            .and_then(|id| self.typed(id, TSK_ANNOTATION_AUTHOR))?;
        let name = first_bytes(&author.payload, AUTHOR_NAME_FIELD)?;
        let name = String::from_utf8_lossy(name).trim().to_string();
        (!name.is_empty()).then_some(name)
    }

    /// `_storage_paragraphs`: one storage's paragraphs, ignoring anything
    /// anchored in it.
    fn storage_paragraphs(&self, identifier: Option<u64>) -> Vec<Paragraph> {
        let Some(storage) = identifier.and_then(|id| self.typed(id, TSWP_STORAGE_ARCHIVE)) else {
            return Vec::new();
        };
        split_paragraphs(
            &storage_text(&storage.payload),
            &storage_runs(&storage.payload, self.objects),
        )
    }

    /// `_drawable_blocks`: whichever kind of drawable `identifier` names.
    fn drawable_blocks(&mut self, identifier: u64) -> Vec<Block> {
        if !self.emitted.insert(identifier) {
            return Vec::new();
        }
        let Some(drawable) = self.object(identifier) else {
            return Vec::new();
        };
        match drawable.ty {
            TSWP_DRAWABLE_ATTACHMENT => {
                match reference_field(&drawable.payload, ATTACHMENT_DRAWABLE_FIELD) {
                    Some(anchored) => self.drawable_blocks(anchored),
                    None => Vec::new(),
                }
            }
            TSD_IMAGE => vec![Block::Picture(self.picture(drawable))],
            TST_TABULAR_INFO => {
                let Some(model) = reference_field(&drawable.payload, TABULAR_INFO_MODEL_FIELD)
                    .and_then(|id| self.typed(id, TST_TABLE_MODEL))
                else {
                    return Vec::new();
                };
                table(model, self.objects)
                    .map(Block::Table)
                    .into_iter()
                    .collect()
            }
            TSD_GROUP => {
                let mut blocks = Vec::new();
                for child in reference_list(&drawable.payload, GROUP_CHILDREN_FIELD) {
                    blocks.extend(self.drawable_blocks(child));
                }
                blocks
            }
            TSWP_SHAPE_INFO => {
                let mut ids = HashSet::new();
                referenced_ids(&drawable.payload, 0, &mut ids);
                let mut ids: Vec<u64> = ids.into_iter().collect();
                ids.sort_unstable();
                let mut blocks = Vec::new();
                for id in ids {
                    if let Some(storage) = self.typed(id, TSWP_STORAGE_ARCHIVE) {
                        blocks.extend(self.storage_blocks(storage));
                    }
                }
                blocks
            }
            _ => Vec::new(),
        }
    }

    /// `_picture`: a `TSD.ImageArchive` and the container member holding its
    /// bytes. Pages names every rendition it knows of, including ones it did
    /// not write into this container, so the renditions are tried in turn.
    fn picture(&mut self, image: &Archive) -> Picture {
        let mut named = String::new();
        for field in IMAGE_DATA_FIELDS {
            let Some(member) = reference_field(&image.payload, field)
                .and_then(|id| self.data_files.get(&id))
                .cloned()
            else {
                continue;
            };
            if named.is_empty() {
                named = member.clone();
            }
            if let Some(data) = self.pkg.read_bytes(&member) {
                return Picture {
                    data: Some(data),
                    name: member,
                };
            }
        }
        Picture {
            data: None,
            name: named,
        }
    }

    /// `iwa_data_files`: each data identifier's container member.
    fn data_files(&self) -> HashMap<u64, String> {
        let mut files = HashMap::new();
        let Some(metadata) = self
            .order
            .iter()
            .filter_map(|id| self.object(*id))
            .find(|a| a.ty == TSP_PACKAGE_METADATA)
        else {
            return files;
        };
        for (f, v) in Fields::new(&metadata.payload) {
            let (PACKAGE_DATAS_FIELD, Value::Bytes(entry)) = (f, v) else {
                continue;
            };
            let name = first_bytes(entry, DATA_INFO_NAME_FIELD)
                .or_else(|| first_bytes(entry, DATA_INFO_PREFERRED_NAME_FIELD));
            if let (Some(identifier), Some(name)) =
                (first_varint(entry, DATA_INFO_IDENTIFIER_FIELD), name)
            {
                files.insert(
                    identifier,
                    format!("{DATA_MEMBER_PREFIX}{}", String::from_utf8_lossy(name)),
                );
            }
        }
        files
    }
}
