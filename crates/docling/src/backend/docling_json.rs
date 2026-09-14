//! docling JSON backend — reads docling's native `DoclingDocument` JSON
//! serialization and re-exports it. docling just pydantic-loads the model; here
//! we walk the `body` tree (children resolved through `$ref` into the
//! `texts`/`groups`/`tables`/`pictures` arrays, skipping `furniture`) and map
//! each item onto the crate's [`Node`] model so the shared Markdown serializer
//! reproduces docling-core's output.

use serde_json::Value;

use crate::backend::markdown::escape_text;
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;
use docling_core::{CaptionParent, DoclingDocument, Node, PictureImage, Table};

pub struct DoclingJsonBackend;

impl DeclarativeBackend for DoclingJsonBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let mut root: Value = serde_json::from_str(source.text()?)
            .map_err(|e| ConversionError::with_source("docling-json", e))?;
        // Referenced images resolve against the JSON file's directory (#403).
        inline_referenced_images(
            &mut root,
            source.path.as_deref().and_then(std::path::Path::parent),
        );
        let root = root;
        let name = root["name"].as_str().unwrap_or(&source.name).to_string();
        let mut doc = DoclingDocument::new(name);
        if let Some(children) = root["body"]["children"].as_array() {
            for c in children {
                walk(c, &root, 0, &mut doc);
            }
        }
        Ok(doc)
    }
}

/// Resolve a `{"$ref": "#/texts/3"}` reference into its array element.
fn resolve<'a>(reference: &Value, root: &'a Value) -> Option<&'a Value> {
    let path = reference["$ref"].as_str()?.strip_prefix("#/")?;
    let (kind, idx) = path.rsplit_once('/')?;
    root.get(kind)?.get(idx.parse::<usize>().ok()?)
}

fn ref_kind(reference: &Value) -> &str {
    reference["$ref"].as_str().unwrap_or("")
}

fn text_of(reference: &Value, root: &Value) -> String {
    resolve(reference, root)
        .map(formatted_text)
        .unwrap_or_default()
}

/// Escaped item text with docling-core's inline markers applied in order:
/// bold → italic → strikethrough → hyperlink (underline/script are no-ops in
/// Markdown).
fn formatted_text(item: &Value) -> String {
    let mut res = escape_text(item["text"].as_str().unwrap_or(""));
    let fmt = &item["formatting"];
    if fmt["bold"].as_bool() == Some(true) {
        res = format!("**{res}**");
    }
    if fmt["italic"].as_bool() == Some(true) {
        res = format!("*{res}*");
    }
    if fmt["strikethrough"].as_bool() == Some(true) {
        res = format!("~~{res}~~");
    }
    if let Some(url) = item["hyperlink"].as_str() {
        res = format!("[{res}]({url})");
    }
    res
}

/// Dispatch one body/child reference by the array it points into.
fn walk(reference: &Value, root: &Value, level: u8, doc: &mut DoclingDocument) {
    let Some(item) = resolve(reference, root) else {
        return;
    };
    if item["content_layer"].as_str() == Some("furniture") {
        return;
    }
    let kind = ref_kind(reference);
    if kind.starts_with("#/texts/") {
        text_item(item, root, level, doc);
        // docling nests a section's content under its heading (and a list
        // item's sub-list under the item), so a text item's `children` are
        // body content too — without this walk a document collapses to its
        // first heading. Tables and pictures are not recursed: their children
        // are rich-cell / caption items already rendered with the parent.
        if let Some(children) = item["children"].as_array() {
            for c in children {
                walk(c, root, level, doc);
            }
        }
    } else if kind.starts_with("#/groups/") {
        group_item(item, root, level, doc);
    } else if kind.starts_with("#/tables/") {
        table_item(item, root, doc);
    } else if kind.starts_with("#/pictures/") {
        picture_item(item, root, doc);
    }
}

fn text_item(item: &Value, root: &Value, level: u8, doc: &mut DoclingDocument) {
    let label = item["label"].as_str().unwrap_or("text");
    // docling does not serialize empty text items (an undecoded formula is the
    // one exception — it becomes a placeholder comment).
    if item["text"].as_str().unwrap_or("").is_empty() {
        if label == "formula" {
            doc.push(Node::Paragraph {
                text: "<!-- formula-not-decoded -->".into(),
            });
        }
        return;
    }
    let text = formatted_text(item);
    // Code and formulas are the two items docling serializes *unescaped*
    // (`escape_html = False`, `escape_underscores = False`): a SQL body keeps
    // its `VERIFY_GROUP_FOR_USER`, not `VERIFY\_GROUP\_FOR\_USER`.
    let raw = || item["text"].as_str().unwrap_or("").to_string();
    match label {
        "title" => doc.push(Node::Heading { level: 1, text }),
        "section_header" => {
            let lvl = item["level"].as_u64().unwrap_or(1) as u8;
            doc.push(Node::Heading {
                level: lvl + 1,
                text,
            });
        }
        "code" => {
            doc.push(Node::Code {
                language: item["code_language"]
                    .as_str()
                    .filter(|s| !s.is_empty() && *s != "unknown")
                    .map(String::from),
                text: raw(),
                orig: None,
                pretty: None,
            });
            // A `CodeItem` is docling's only *floating* text item, and the
            // text serializer appends a floating item's captions **after** its
            // own text — the opposite of a picture or a table, which lead with
            // theirs (`Listing 1: …` under the fence, not above it).
            if let Some(cap) = caption_of(item, root) {
                doc.push(Node::Paragraph { text: cap });
            }
        }
        "list_item" => doc.push(Node::ListItem {
            ordered: item["enumerated"].as_bool().unwrap_or(false),
            number: 1,
            first_in_list: true,
            text,
            level,
            marker: None,
            location: None,
            dclx: None,
            href: None,
            layer: None,
        }),
        // docling prefixes the task-list marker to the text of a checkbox item.
        "checkbox_selected" => doc.push(Node::CheckboxItem {
            checked: true,
            text,
        }),
        "checkbox_unselected" => doc.push(Node::CheckboxItem {
            checked: false,
            text,
        }),
        // A decoded formula renders as `$$…$$`, also unescaped.
        "formula" => doc.push(Node::Formula {
            latex: raw(),
            orig: item["orig"].as_str().unwrap_or("").to_string(),
            location: None,
        }),
        // A caption some table/picture/code claims renders with that element;
        // one nobody claims is an ordinary body item and renders where it sits
        // (docling's serializer only skips the refs a floating item consumed).
        "caption" => {
            if !caption_is_claimed(item, root) {
                doc.push(Node::Caption { text, href: None });
            }
        }
        _ => doc.push(Node::Paragraph { text }), // text, paragraph, footnote, …
    }
}

fn group_item(item: &Value, root: &Value, level: u8, doc: &mut DoclingDocument) {
    let label = item["label"].as_str().unwrap_or("unspecified");
    let empty = Vec::new();
    let children = item["children"].as_array().unwrap_or(&empty);
    match label {
        "list" | "ordered_list" => list_group(children, root, level, doc),
        "inline" => {
            // An inline group is one line: serialize each child and join with " ".
            let joined = children
                .iter()
                .map(|c| text_of(c, root))
                .collect::<Vec<_>>()
                .join(" ");
            if !joined.is_empty() {
                doc.push(Node::Paragraph { text: joined });
            }
        }
        // section / chapter / unspecified / sheet / comment_section → transparent
        _ => {
            for c in children {
                walk(c, root, level, doc);
            }
        }
    }
}

/// How docling-core's Markdown serializer renders a list item's marker.
///
/// * A marker it already considers valid Markdown — a bullet, or `12.` — is
///   printed **verbatim** and nothing is computed. This is what carries a
///   split reference list's real numbering (`18.` on the group that continues
///   over a page break) instead of restarting at 1.
/// * Any *other* non-empty marker (`a.`, `[7]`, `(3)`) forces a **bullet**:
///   the computed marker is a number only when the item carries no marker at
///   all (`… and (mode != AUTO or not item.marker)`). The original marker is
///   then kept after it when it holds a letter or digit, so docling renders
///   `- (1) Human Annotation`. In this node model it rides in the item's text,
///   which is where it lands on the rendered line either way; a marker with
///   no alphanumerics (a stray bullet glyph) is dropped, as upstream drops it.
/// * No marker at all leaves the group to decide: `{position}.` when its first
///   child is an enumerated item, `-` otherwise.
enum Marker {
    /// Print verbatim: `Some(n)` numbers the item, `None` bullets it.
    Verbatim(Option<u64>),
    /// Bullet, with this text (if any) kept in front of the item's own.
    Bullet(Option<String>),
    /// Nothing of its own — the group's kind and the item's position decide.
    FromGroup,
}

fn marker_of(item: &Value) -> Marker {
    let raw = item["marker"].as_str().unwrap_or("");
    if raw.is_empty() {
        return Marker::FromGroup;
    }
    if matches!(raw, "-" | "*" | "+") {
        return Marker::Verbatim(None);
    }
    if let Some(digits) = raw.strip_suffix('.') {
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            return Marker::Verbatim(digits.parse().ok());
        }
    }
    Marker::Bullet(
        raw.chars()
            .any(|c| c.is_ascii_alphanumeric())
            .then(|| raw.to_string()),
    )
}

/// Emit a list group's items, recursing into nested lists at the next level.
fn list_group(children: &[Value], root: &Value, level: u8, doc: &mut DoclingDocument) {
    // docling-core's `first_item_is_enumerated`: the group renders computed
    // markers as numbers only when its *first child* is an enumerated item.
    let enumerated_group = children
        .first()
        .and_then(|c| resolve(c, root))
        .is_some_and(|it| {
            it["label"].as_str() == Some("list_item") && it["enumerated"].as_bool() == Some(true)
        });
    let mut first = true;
    for (pos, c) in children.iter().enumerate() {
        let kind = ref_kind(c);
        if kind.starts_with("#/groups/") {
            // A bare nested list (no enclosing item). It still occupies a
            // position in this group, which is why a later item can be
            // numbered past it.
            walk(c, root, level + 1, doc);
            continue;
        }
        let Some(item) = resolve(c, root) else {
            continue;
        };
        if item["label"].as_str() != Some("list_item") {
            continue;
        }
        let position = pos as u64 + 1;
        let body = formatted_text(item);
        let (ordered, number, text) = match marker_of(item) {
            Marker::Verbatim(Some(n)) => (true, n, body),
            Marker::Verbatim(None) => (false, position, body),
            Marker::Bullet(kept) => {
                let text = match kept {
                    Some(m) if !body.is_empty() => format!("{m} {body}"),
                    Some(m) => m,
                    None => body,
                };
                (false, position, text)
            }
            Marker::FromGroup => (enumerated_group, position, body),
        };
        doc.push(Node::ListItem {
            ordered,
            number,
            first_in_list: first,
            text,
            level,
            marker: None,
            location: None,
            dclx: None,
            href: None,
            layer: None,
        });
        first = false;
        if let Some(sub) = item["children"].as_array() {
            for s in sub {
                walk(s, root, level + 1, doc);
            }
        }
    }
}

fn table_item(item: &Value, root: &Value, doc: &mut DoclingDocument) {
    let mut rows = Vec::new();
    // Per-cell flags carried through so DocTags export and the chunker's
    // triplet serialization see docling's header/span structure, not just the
    // flattened text grid.
    let mut structure = docling_core::TableStructure::default();
    if let Some(grid) = item["data"]["grid"].as_array() {
        for (r, row) in grid.iter().enumerate() {
            let Some(cells) = row.as_array() else {
                continue;
            };
            rows.push(
                cells
                    .iter()
                    .map(|cell| cell["text"].as_str().unwrap_or("").to_string())
                    .collect::<Vec<_>>(),
            );
            // A spanning cell is repeated at each grid position it covers; its
            // start offsets mark the anchor, everything past it a continuation.
            let past = |cell: &Value, key: &str, idx: usize| {
                cell[key].as_u64().is_some_and(|v| (v as usize) < idx)
            };
            structure
                .col_header
                .push(flags(cells, |c| c["column_header"].as_bool() == Some(true)));
            structure
                .row_header
                .push(flags(cells, |c| c["row_header"].as_bool() == Some(true)));
            structure.col_continuation.push(flags(cells.as_slice(), {
                let mut col = 0;
                move |c| {
                    let cont = past(c, "start_col_offset_idx", col);
                    col += 1;
                    cont
                }
            }));
            structure
                .row_continuation
                .push(flags(cells, |c| past(c, "start_row_offset_idx", r)));
        }
    }
    if !rows.is_empty() {
        let has_structure = structure.col_header.iter().flatten().any(|&b| b)
            || structure.row_header.iter().flatten().any(|&b| b)
            || structure.col_continuation.iter().flatten().any(|&b| b)
            || structure.row_continuation.iter().flatten().any(|&b| b);
        doc.push(Node::Table(Table {
            rows,
            location: None,
            structure: has_structure.then_some(structure),
            cell_blocks: None,
            cells: None,
            caption: caption_of(item, root),
            caption_parent: caption_parent_of(item, root),
        }));
    }
}

/// Map each grid cell to a flag.
fn flags(cells: &[Value], f: impl FnMut(&Value) -> bool) -> Vec<bool> {
    cells.iter().map(f).collect()
}

fn picture_item(item: &Value, root: &Value, doc: &mut DoclingDocument) {
    doc.push(Node::Picture {
        caption: caption_of(item, root),
        caption_parent: caption_parent_of(item, root),
        caption_href: None,
        image: picture_image(item),
        classification: None,
    });
}

/// Where the item's first caption hangs in the source tree, so a re-export
/// keeps it there (#390): on the item (a PDF layout caption), on the item's
/// own parent — ahead of the item or behind it, by the parent's `children`
/// order — or, anywhere else, docling's `add_text` default, the body.
fn caption_parent_of(item: &Value, root: &Value) -> CaptionParent {
    let cap = &item["captions"][0];
    let Some(cap_ref) = cap["$ref"].as_str() else {
        return CaptionParent::Body;
    };
    let cap_parent = resolve(cap, root)
        .and_then(|c| c["parent"]["$ref"].as_str())
        .unwrap_or("#/body");
    let self_ref = item["self_ref"].as_str().unwrap_or("");
    let item_parent = &item["parent"];
    if cap_parent == self_ref {
        CaptionParent::Item
    } else if cap_parent == ref_kind(item_parent) && cap_parent != "#/body" {
        let pos = |r: &str| {
            resolve(item_parent, root)?
                .get("children")?
                .as_array()?
                .iter()
                .position(|c| c["$ref"] == r)
        };
        match (pos(cap_ref), pos(self_ref)) {
            (Some(c), Some(i)) if c > i => CaptionParent::ContainerAfter,
            _ => CaptionParent::Container,
        }
    } else {
        CaptionParent::Body
    }
}

/// The picture's embedded image — docling's `ImageRef` with a `data:` URI
/// (`{"mimetype", "dpi", "size", "uri"}`), which is how docling and our own
/// JSON export carry a picture's pixels. Without it every picture read back
/// from JSON was an empty placeholder, and `--images embedded`/`referenced`
/// silently had nothing to embed or write (#403). The dimensions come from the
/// bytes themselves (PNG IHDR / JPEG SOF), the declared `size` as a fallback.
fn picture_image(item: &Value) -> Option<PictureImage> {
    let image = item.get("image")?;
    let uri = image["uri"].as_str()?;
    let (mimetype, data) = parse_data_uri(uri)?;
    let mimetype = image["mimetype"]
        .as_str()
        .filter(|m| !m.is_empty())
        .unwrap_or(&mimetype)
        .to_string();
    let declared = |k: &str| image["size"][k].as_f64().unwrap_or(0.0).round().max(0.0) as u32;
    let (width, height) = crate::backend::rtf::image_size(&mimetype, &data)
        .unwrap_or((declared("width"), declared("height")));
    Some(PictureImage {
        mimetype,
        width,
        height,
        data,
    })
}

/// Split a `data:<mimetype>;base64,<payload>` URI into its media type and
/// decoded bytes. Anything else — a plain path, an `http(s)` or `file` URL —
/// is `None`: [`DoclingJsonBackend::convert`] inlines the local files it can
/// read beforehand, so only a genuinely unresolvable reference reaches here.
fn parse_data_uri(uri: &str) -> Option<(String, Vec<u8>)> {
    let rest = uri.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let (mimetype, encoding) = meta.split_once(';').unwrap_or((meta, ""));
    if !encoding.eq_ignore_ascii_case("base64") {
        return None;
    }
    let data = docling_core::base64::decode(payload.trim())?;
    Some((mimetype.to_string(), data))
}

/// Turn each picture's *referenced* image (`uri` a filesystem path or `file:`
/// URL, as `--images referenced` writes them) into a `data:` URI, reading the
/// file relative to the JSON's own directory — docling's `ImageRef.pil_image`
/// opens such a URI from disk the same way. A file that cannot be read keeps
/// its `uri`, so the picture degrades to a placeholder rather than failing.
fn inline_referenced_images(root: &mut Value, base: Option<&std::path::Path>) {
    let Some(pictures) = root.get_mut("pictures").and_then(Value::as_array_mut) else {
        return;
    };
    for pic in pictures {
        let Some(image) = pic.get_mut("image").filter(|i| i.is_object()) else {
            continue;
        };
        let Some(uri) = image["uri"].as_str().map(str::to_string) else {
            continue;
        };
        if uri.starts_with("data:") || (uri.contains("://") && !uri.starts_with("file://")) {
            continue;
        }
        let path = std::path::PathBuf::from(uri.strip_prefix("file://").unwrap_or(&uri));
        let path = match base {
            Some(dir) if path.is_relative() => dir.join(path),
            _ => path,
        };
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        let mimetype = image["mimetype"]
            .as_str()
            .filter(|m| !m.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| mime_of_path(&path));
        image["uri"] = Value::String(format!(
            "data:{mimetype};base64,{}",
            docling_core::base64::encode(&data)
        ));
    }
}

/// A media type from a file extension, for a referenced image whose `ImageRef`
/// carries none.
fn mime_of_path(path: &std::path::Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        Some("webp") => "image/webp",
        Some("tif") | Some("tiff") => "image/tiff",
        _ => "image/png",
    }
    .to_string()
}

/// Whether a floating item lists this caption in its own `captions` — tables,
/// pictures and code items are the three that can. Scanning per caption keeps
/// the walk's signature unchanged; a document has few of either.
fn caption_is_claimed(item: &Value, root: &Value) -> bool {
    let Some(me) = item["self_ref"].as_str() else {
        return false;
    };
    ["tables", "pictures", "texts"].iter().any(|bucket| {
        root[bucket].as_array().is_some_and(|items| {
            items.iter().any(|it| {
                it["captions"]
                    .as_array()
                    .is_some_and(|caps| caps.iter().any(|c| c["$ref"].as_str() == Some(me)))
            })
        })
    })
}

/// An item's `captions` (refs into `texts`), joined as docling-core joins them
/// (`caption_delim`, a space). It belongs *on* the table or picture, not after
/// it: every serializer renders a caption before its element, and emitting the
/// caption items as trailing paragraphs put them on the wrong side (#384).
fn caption_of(item: &Value, root: &Value) -> Option<String> {
    let caps = item["captions"].as_array()?;
    let joined = caps
        .iter()
        .map(|c| text_of(c, root))
        .collect::<Vec<_>>()
        .join(" ");
    (!joined.trim().is_empty()).then_some(joined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::InputFormat;

    fn md(json: &str) -> String {
        DoclingJsonBackend
            .convert(&SourceDocument::from_bytes(
                "t.json",
                InputFormat::JsonDocling,
                json.as_bytes().to_vec(),
            ))
            .unwrap()
            .export_to_markdown()
    }

    fn item(self_ref: &str, parent: &str, label: &str, extra: &str) -> String {
        format!(
            r#"{{"self_ref":"{self_ref}","parent":{{"$ref":"{parent}"}},"children":[],"label":"{label}","prov":[]{extra}}}"#
        )
    }

    /// #390: where a caption hangs is read back from the source tree, so a
    /// re-export keeps a PDF caption on its picture, an office chart's on the
    /// sheet ahead of the picture, and a declarative backend's on the body.
    #[test]
    fn caption_parents_round_trip() {
        let cap = |i: usize, parent: &str| {
            item(
                &format!("#/texts/{i}"),
                parent,
                "caption",
                r#","orig":"c","text":"c""#,
            )
        };
        let pic = |i: usize, parent: &str, cap: usize| {
            item(
                &format!("#/pictures/{i}"),
                parent,
                "picture",
                &format!(r##","captions":[{{"$ref":"#/texts/{cap}"}}],"annotations":[]"##),
            )
        };
        let json = format!(
            r##"{{"schema_name":"DoclingDocument","version":"1.10.0","name":"t",
            "body":{{"self_ref":"#/body","children":[{{"$ref":"#/pictures/0"}},{{"$ref":"#/groups/0"}},{{"$ref":"#/texts/1"}},{{"$ref":"#/pictures/2"}}],"name":"_root_","label":"unspecified"}},
            "furniture":{{"self_ref":"#/furniture","children":[],"name":"_root_","label":"unspecified"}},
            "groups":[{{"self_ref":"#/groups/0","parent":{{"$ref":"#/body"}},"children":[{{"$ref":"#/texts/2"}},{{"$ref":"#/pictures/1"}}],"name":"sheet","label":"section"}}],
            "texts":[{},{},{}],
            "pictures":[{},{},{}],
            "tables":[],"key_value_items":[],"form_items":[],"pages":{{}}}}"##,
            cap(0, "#/pictures/0"),
            cap(1, "#/body"),
            cap(2, "#/groups/0"),
            pic(0, "#/body", 0),
            pic(1, "#/groups/0", 2),
            pic(2, "#/body", 1),
        );
        let doc = DoclingJsonBackend
            .convert(&SourceDocument::from_bytes(
                "t.json",
                InputFormat::JsonDocling,
                json.into_bytes(),
            ))
            .unwrap();
        let parents: Vec<CaptionParent> = doc
            .nodes
            .iter()
            .filter_map(|n| match n {
                Node::Picture { caption_parent, .. } => Some(*caption_parent),
                _ => None,
            })
            .collect();
        assert_eq!(
            parents,
            [
                CaptionParent::Item,
                CaptionParent::Container,
                CaptionParent::Body
            ]
        );
        let v: Value = serde_json::from_str(&doc.export_to_json()).unwrap();
        let parent = |i: usize| {
            v["texts"][i]["parent"]["$ref"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(parent(0), "#/pictures/0");
        // A `section` group is replayed transparently (only lists come back
        // as groups), so the container caption lands beside its picture on
        // the body — ahead of it, as a chart's title is.
        assert_eq!(parent(1), "#/body");
        assert_eq!(parent(2), "#/body");
        let body: Vec<&str> = v["body"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["$ref"].as_str().unwrap())
            .collect();
        assert_eq!(
            body,
            [
                "#/pictures/0",
                "#/texts/1",
                "#/pictures/1",
                "#/texts/2",
                "#/pictures/2"
            ]
        );
    }

    /// #403: a picture's `image` (docling's `ImageRef`, a `data:` URI) comes
    /// back as the node's image — so `--images embedded`/`referenced` have
    /// pixels to work with — and a *referenced* image (a path relative to the
    /// JSON file, as `--images referenced` writes) is read from disk.
    #[test]
    fn a_pictures_image_survives_the_round_trip() {
        const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAADwAAAAoCAIAAAAt2Q6oAAAASUlEQVR4nO3OQQ3AIAAAMUAI/qUgCw97HFnSKug8e4+/Wa8DX0hXpCvSFemKdEW6Il2RrkhXpCvSFemKdEW6Il2RrkhXpCvSlQtBOQFUzKPJpQAAAABJRU5ErkJggg==";
        let json_with = |uri: &str| {
            format!(
                r##"{{"name":"t","body":{{"children":[{{"$ref":"#/pictures/0"}}]}},
                  "texts":[],"groups":[],"tables":[],
                  "pictures":[{{"self_ref":"#/pictures/0","label":"picture","captions":[],"children":[],
                    "image":{{"mimetype":"image/png","dpi":72,"size":{{"width":60.0,"height":40.0}},"uri":"{uri}"}}}}]}}"##
            )
        };
        let image_of = |doc: &DoclingDocument| {
            doc.nodes
                .iter()
                .find_map(|n| match n {
                    Node::Picture { image, .. } => image.clone(),
                    _ => None,
                })
                .expect("the picture keeps its image")
        };
        let doc = DoclingJsonBackend
            .convert(&SourceDocument::from_bytes(
                "t.json",
                InputFormat::JsonDocling,
                json_with(&format!("data:image/png;base64,{PNG}")).into_bytes(),
            ))
            .unwrap();
        let img = image_of(&doc);
        assert_eq!(
            (img.mimetype.as_str(), img.width, img.height),
            ("image/png", 60, 40)
        );
        assert_eq!(img.data_uri(), format!("data:image/png;base64,{PNG}"));
        assert!(doc
            .export_to_markdown_with_images(docling_core::ImageMode::Embedded, "artifacts")
            .0
            .contains(&format!("![Image](data:image/png;base64,{PNG})")));

        // Referenced: the JSON sits next to its `artifacts/` directory.
        let dir = std::env::temp_dir().join(format!(
            "docling-rs-json-pic-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("artifacts")).unwrap();
        std::fs::write(
            dir.join("artifacts/image_000000.png"),
            docling_core::base64::decode(PNG).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("t.json"), json_with("artifacts/image_000000.png")).unwrap();
        let doc = DoclingJsonBackend
            .convert(&SourceDocument::from_file(dir.join("t.json")).unwrap())
            .unwrap();
        let img = image_of(&doc);
        assert_eq!((img.width, img.height), (60, 40));
        assert_eq!(img.data_uri(), format!("data:image/png;base64,{PNG}"));
        // An unreadable reference degrades to a placeholder, never an error.
        std::fs::write(dir.join("t.json"), json_with("artifacts/missing.png")).unwrap();
        let doc = DoclingJsonBackend
            .convert(&SourceDocument::from_file(dir.join("t.json")).unwrap())
            .unwrap();
        assert!(doc
            .nodes
            .iter()
            .any(|n| matches!(n, Node::Picture { image: None, .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #384: a caption leads its picture and its table, and trails its *code*
    /// block — a `CodeItem` is docling's only floating text item, and the text
    /// serializer appends a floating item's captions after its own text.
    #[test]
    fn captions_lead_a_picture_and_a_table_but_trail_code() {
        let json = r##"{
          "name":"n","body":{"children":[{"$ref":"#/pictures/0"},{"$ref":"#/tables/0"},{"$ref":"#/texts/2"}]},
          "texts":[
            {"self_ref":"#/texts/0","label":"caption","text":"Figure 1: a duck","children":[]},
            {"self_ref":"#/texts/1","label":"caption","text":"Table 1: the counts","children":[]},
            {"self_ref":"#/texts/2","label":"code","text":"let x = 1;","children":[],
             "captions":[{"$ref":"#/texts/3"}]},
            {"self_ref":"#/texts/3","label":"caption","text":"Listing 1: a binding","children":[]}
          ],
          "groups":[],
          "tables":[{"self_ref":"#/tables/0","label":"table","captions":[{"$ref":"#/texts/1"}],
                     "data":{"grid":[[{"text":"a"}]]},"children":[]}],
          "pictures":[{"self_ref":"#/pictures/0","label":"picture","captions":[{"$ref":"#/texts/0"}],"children":[]}]
        }"##;
        assert_eq!(
            md(json),
            "Figure 1: a duck\n\n<!-- image -->\n\nTable 1: the counts\n\n| a   |\n|-----|\n\n```\nlet x = 1;\n```\n\nListing 1: a binding\n"
        );
    }

    /// A caption no floating item claims is an ordinary body item and renders
    /// where it sits; one that is claimed renders only with its element.
    #[test]
    fn an_unclaimed_caption_still_renders() {
        let json = r##"{
          "name":"n","body":{"children":[{"$ref":"#/texts/0"},{"$ref":"#/pictures/0"},{"$ref":"#/texts/1"}]},
          "texts":[
            {"self_ref":"#/texts/0","label":"caption","text":"Figure 6: nobody claims me","children":[]},
            {"self_ref":"#/texts/1","label":"caption","text":"Figure 7: claimed","children":[]}
          ],
          "groups":[],"tables":[],
          "pictures":[{"self_ref":"#/pictures/0","label":"picture","captions":[{"$ref":"#/texts/1"}],"children":[]}]
        }"##;
        assert_eq!(
            md(json),
            "Figure 6: nobody claims me\n\nFigure 7: claimed\n\n<!-- image -->\n"
        );
    }

    /// docling-core prints a marker it already considers valid Markdown
    /// verbatim, so a reference list that continues over a page break keeps its
    /// real numbering instead of restarting at 1.
    #[test]
    fn a_numeric_marker_is_printed_verbatim() {
        let json = r##"{
          "name":"n","body":{"children":[{"$ref":"#/groups/0"}]},
          "texts":[
            {"self_ref":"#/texts/0","label":"list_item","text":"Xue, W.","marker":"18.","enumerated":true,"children":[]},
            {"self_ref":"#/texts/1","label":"list_item","text":"Ye, J.","marker":"19.","enumerated":true,"children":[]}
          ],
          "groups":[{"self_ref":"#/groups/0","label":"list","name":"list",
                     "children":[{"$ref":"#/texts/0"},{"$ref":"#/texts/1"}]}],
          "tables":[],"pictures":[]
        }"##;
        assert_eq!(md(json), "18. Xue, W.\n19. Ye, J.\n");
    }

    /// Any *other* non-empty marker forces a bullet and is kept in front of the
    /// text when it holds a letter or digit (docling: `- (1) Human Annotation`),
    /// because a number is computed only for an item with no marker at all.
    #[test]
    fn a_non_markdown_marker_forces_a_bullet_and_is_kept() {
        let json = r##"{
          "name":"n","body":{"children":[{"$ref":"#/groups/0"}]},
          "texts":[
            {"self_ref":"#/texts/0","label":"list_item","text":"Human Annotation","marker":"(1)","enumerated":true,"children":[]},
            {"self_ref":"#/texts/1","label":"list_item","text":"Red - PDF cells","marker":"a.","enumerated":true,"children":[]},
            {"self_ref":"#/texts/2","label":"list_item","text":"a stray glyph","marker":"\u0084","enumerated":false,"children":[]}
          ],
          "groups":[{"self_ref":"#/groups/0","label":"list","name":"list",
                     "children":[{"$ref":"#/texts/0"},{"$ref":"#/texts/1"},{"$ref":"#/texts/2"}]}],
          "tables":[],"pictures":[]
        }"##;
        assert_eq!(
            md(json),
            "- (1) Human Annotation\n- a. Red - PDF cells\n- a stray glyph\n"
        );
    }

    /// With no marker at all the group decides: its first child being an
    /// enumerated item numbers every item by its *position among the children*,
    /// nested groups included — so an item after a sublist is numbered past it.
    #[test]
    fn an_unmarked_item_is_numbered_by_its_position_in_the_group() {
        let json = r##"{
          "name":"n","body":{"children":[{"$ref":"#/groups/0"}]},
          "texts":[
            {"self_ref":"#/texts/0","label":"list_item","text":"one","marker":"","enumerated":true,"children":[]},
            {"self_ref":"#/texts/1","label":"list_item","text":"nested","marker":"","enumerated":true,"children":[]},
            {"self_ref":"#/texts/2","label":"list_item","text":"after","marker":"","enumerated":true,"children":[]}
          ],
          "groups":[
            {"self_ref":"#/groups/0","label":"list","name":"list",
             "children":[{"$ref":"#/texts/0"},{"$ref":"#/groups/1"},{"$ref":"#/texts/2"}]},
            {"self_ref":"#/groups/1","label":"list","name":"list","children":[{"$ref":"#/texts/1"}]}
          ],
          "tables":[],"pictures":[]
        }"##;
        // The nested group takes position 2, so "after" is the third child.
        assert_eq!(md(json), "1. one\n    1. nested\n3. after\n");
    }

    /// A group whose first child is not an enumerated item bullets every item,
    /// whatever each one's own flag says.
    #[test]
    fn a_group_starting_on_a_bullet_stays_bulleted() {
        let json = r##"{
          "name":"n","body":{"children":[{"$ref":"#/groups/0"}]},
          "texts":[
            {"self_ref":"#/texts/0","label":"list_item","text":"bullet first","marker":"","enumerated":false,"children":[]},
            {"self_ref":"#/texts/1","label":"list_item","text":"still a bullet","marker":"","enumerated":true,"children":[]}
          ],
          "groups":[{"self_ref":"#/groups/0","label":"list","name":"list",
                     "children":[{"$ref":"#/texts/0"},{"$ref":"#/texts/1"}]}],
          "tables":[],"pictures":[]
        }"##;
        assert_eq!(md(json), "- bullet first\n- still a bullet\n");
    }

    /// Checkbox items carry docling's task-list marker, and code and formulas
    /// are the two items it serializes unescaped.
    #[test]
    fn checkboxes_render_and_code_is_not_escaped() {
        let json = r##"{
          "name":"n","body":{"children":[{"$ref":"#/texts/0"},{"$ref":"#/texts/1"},{"$ref":"#/texts/2"},{"$ref":"#/texts/3"}]},
          "texts":[
            {"self_ref":"#/texts/0","label":"checkbox_selected","text":"done","children":[]},
            {"self_ref":"#/texts/1","label":"checkbox_unselected","text":"todo","children":[]},
            {"self_ref":"#/texts/2","label":"code","text":"VERIFY_GROUP_FOR_USER ( SESSION_USER )","children":[]},
            {"self_ref":"#/texts/3","label":"formula","text":"a_1 + b_2","orig":"a_1 + b_2","children":[]}
          ],
          "groups":[],"tables":[],"pictures":[]
        }"##;
        assert_eq!(
            md(json),
            "- [x] done\n\n- [ ] todo\n\n```\nVERIFY_GROUP_FOR_USER ( SESSION_USER )\n```\n\n$$a_1 + b_2$$\n"
        );
    }

    /// docling ≥ 2.5x nests body content under its `section_header`; the
    /// nested items must be walked, not dropped with the heading's subtree.
    #[test]
    fn walks_children_nested_under_headings() {
        let json = r##"{
          "name": "n", "body": {"children": [{"$ref":"#/texts/0"}]},
          "texts": [
            {"self_ref":"#/texts/0","label":"section_header","level":1,"text":"Intro",
             "children":[{"$ref":"#/texts/1"},{"$ref":"#/texts/2"}]},
            {"self_ref":"#/texts/1","label":"text","text":"First para","children":[]},
            {"self_ref":"#/texts/2","label":"section_header","level":2,"text":"Sub",
             "children":[{"$ref":"#/texts/3"}]},
            {"self_ref":"#/texts/3","label":"text","text":"Deep para","children":[]}
          ],
          "groups": [], "tables": [], "pictures": []
        }"##;
        let doc = DoclingJsonBackend
            .convert(&SourceDocument::from_bytes(
                "t.json",
                InputFormat::JsonDocling,
                json.as_bytes().to_vec(),
            ))
            .unwrap();
        assert_eq!(
            doc.export_to_markdown(),
            "## Intro\n\nFirst para\n\n### Sub\n\nDeep para\n"
        );
    }

    #[test]
    fn walks_body_tree_with_formatting_and_lists() {
        let json = r##"{
          "schema_name": "DoclingDocument", "name": "t",
          "body": {"children": [{"$ref":"#/texts/0"},{"$ref":"#/texts/1"},{"$ref":"#/groups/0"}]},
          "texts": [
            {"self_ref":"#/texts/0","label":"title","text":"Doc"},
            {"self_ref":"#/texts/1","label":"section_header","level":1,"text":"Sec","hyperlink":"http://x"},
            {"self_ref":"#/texts/2","label":"list_item","text":"one","enumerated":false},
            {"self_ref":"#/texts/3","label":"list_item","text":"two","enumerated":false,
             "formatting":{"bold":true,"italic":false,"strikethrough":false}}
          ],
          "groups": [{"self_ref":"#/groups/0","label":"list",
                      "children":[{"$ref":"#/texts/2"},{"$ref":"#/texts/3"}]}],
          "tables": [], "pictures": []
        }"##;
        let src =
            SourceDocument::from_bytes("t", InputFormat::JsonDocling, json.as_bytes().to_vec());
        let md = DoclingJsonBackend
            .convert(&src)
            .unwrap()
            .export_to_markdown();
        assert!(
            md.starts_with("# Doc\n\n## [Sec](http://x)\n\n- one\n- **two**"),
            "got:\n{md}"
        );
    }
}
