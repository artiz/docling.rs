//! Export a [`DoclingDocument`] to docling-core's native JSON wire format
//! (`DoclingDocument` schema v1.10.0) — the same shape `export_to_dict()` /
//! `save_as_json()` produce in Python docling, and the inverse of the
//! JSON-docling reader.
//!
//! The crate's [`Node`] model bakes Markdown escaping (and inline markers) into
//! its text, whereas docling stores raw text and escapes at render time. We
//! therefore *un-escape* on the way out so a docling-core round-trip
//! (`load_from_json().export_to_markdown()`) reproduces the same Markdown.

use serde_json::{json, Value};

use crate::document::{DoclingDocument, Node, Table};

const SCHEMA_VERSION: &str = "1.10.0";

/// docling-core's `CodeLanguageLabel` values (anything else serializes as
/// `unknown`, which the model requires for code items).
const CODE_LANGUAGES: &[&str] = &[
    "Ada",
    "Awk",
    "Bash",
    "bc",
    "C",
    "C#",
    "C++",
    "CMake",
    "COBOL",
    "CSS",
    "Ceylon",
    "Clojure",
    "Crystal",
    "Cuda",
    "Cython",
    "D",
    "Dart",
    "dc",
    "Dockerfile",
    "DocLang",
    "Elixir",
    "Erlang",
    "FORTRAN",
    "Forth",
    "Go",
    "HTML",
    "Haskell",
    "Haxe",
    "Java",
    "JavaScript",
    "JSON",
    "Julia",
    "Kotlin",
    "Latex",
    "Lisp",
    "Lua",
    "Matlab",
    "MoonScript",
    "Nim",
    "OCaml",
    "ObjectiveC",
    "Octave",
    "PHP",
    "Pascal",
    "Perl",
    "Prolog",
    "Python",
    "Racket",
    "Ruby",
    "Rust",
    "SML",
    "SQL",
    "Scala",
    "Scheme",
    "Swift",
    "Tikz",
    "TypeScript",
    "VisualBasic",
    "XML",
    "YAML",
];

/// Map a fence language to docling's `CodeLanguageLabel` (case-insensitive), else
/// `unknown`.
pub(crate) fn code_language(lang: Option<&str>) -> &'static str {
    match lang {
        Some(l) => CODE_LANGUAGES
            .iter()
            .find(|c| c.eq_ignore_ascii_case(l))
            .copied()
            .unwrap_or("unknown"),
        None => "unknown",
    }
}

/// Build the docling-core JSON object for `doc`.
pub fn to_json(doc: &DoclingDocument) -> Value {
    let mut b = Builder::default();
    let body = b.walk_into(&doc.nodes, "#/body");
    b.link_comments();

    let mut out = json!({
        "schema_name": "DoclingDocument",
        "version": SCHEMA_VERSION,
        "name": doc.name,
        "origin": {
            "mimetype": "text/plain",
            "binary_hash": fnv1a(&doc.name),
            "filename": doc.name,
        },
        "furniture": {
            "self_ref": "#/furniture",
            "children": [],
            "content_layer": "furniture",
            "name": "_root_",
            "label": "unspecified",
        },
        "body": {
            "self_ref": "#/body",
            "children": body,
            "content_layer": "body",
            "name": "_root_",
            "label": "unspecified",
        },
        "groups": b.groups,
        "texts": b.texts,
        "pictures": b.pictures,
        "tables": b.tables,
        "key_value_items": [],
        "form_items": [],
        "pages": b.pages.iter().map(|(n, w, h)| {
            let r2 = |v: f64| (v * 100.0).round() / 100.0;
            (n.to_string(), json!({
                "size": { "width": r2(*w), "height": r2(*h) },
                "page_no": n,
            }))
        }).collect::<serde_json::Map<String, Value>>(),
    });

    // docling only emits `field_regions` / `field_items` when a document has
    // form fields, and places them just before `pages`. Insert them in that slot
    // (re-appending `pages` afterwards, since `preserve_order` keeps insertion
    // order) so non-KVP documents' JSON is byte-identical to before.
    if !b.field_regions.is_empty() {
        if let Some(obj) = out.as_object_mut() {
            let pages = obj.remove("pages");
            obj.insert("field_regions".into(), Value::Array(b.field_regions));
            obj.insert("field_items".into(), Value::Array(b.field_items));
            if let Some(pages) = pages {
                obj.insert("pages".into(), pages);
            }
        }
    }
    out
}

#[derive(Default)]
struct Builder {
    texts: Vec<Value>,
    groups: Vec<Value>,
    tables: Vec<Value>,
    pictures: Vec<Value>,
    field_regions: Vec<Value>,
    field_items: Vec<Value>,
    /// Pages seen so far (`page_no`, width, height in points) — from the
    /// [`Node::PageInfo`] markers the PDF paths emit; empty for every other
    /// backend, which keeps their JSON byte-identical (`"pages": {}`, no prov).
    pages: Vec<(usize, f64, f64)>,
    /// The page the walk is currently on (0 before the first marker).
    cur_page: usize,
    cur_w: f64,
    cur_h: f64,
    /// The enclosing [`Node::Located`] wrapper's 0–511 grid box, waiting to be
    /// consumed as the next item's provenance.
    pending_loc: Option<[u16; 4]>,
    /// Each [`Node::CommentSection`]'s group `$ref`, in document order — the
    /// index a [`Node::Commented`] annotation refers to.
    comment_groups: Vec<String>,
    /// Annotated items awaiting their refs: comments are usually emitted
    /// *after* the body they annotate (docx appends them), so the link is
    /// patched in once the whole document has been walked.
    pending_comments: Vec<(String, Vec<usize>)>,
}

impl Builder {
    /// Consume the pending location (if any) into a docling `prov` array: the
    /// 0-511 grid denormalized against the current page into BOTTOMLEFT
    /// points, rounded to 2 decimals like docling's own export. `char_len` is
    /// the item's text length in characters (0 for tables and pictures, whose
    /// charspan docling emits as `[0, 0]`).
    fn take_prov(&mut self, char_len: usize) -> Value {
        let Some([x0, y0, x1, y1]) = self.pending_loc.take() else {
            return json!([]);
        };
        let r2 = |v: f64| (v * 100.0).round() / 100.0;
        json!([{
            "page_no": self.cur_page,
            "bbox": {
                "l": r2(x0 as f64 * self.cur_w / 512.0),
                "t": r2(self.cur_h - y0 as f64 * self.cur_h / 512.0),
                "r": r2(x1 as f64 * self.cur_w / 512.0),
                "b": r2(self.cur_h - y1 as f64 * self.cur_h / 512.0),
                "coord_origin": "BOTTOMLEFT",
            },
            "charspan": [0, char_len],
        }])
    }

    /// Adopt a node's own location field (tables, formulas, list items carry
    /// one instead of a [`Node::Located`] wrapper) when no wrapper is pending.
    fn adopt_loc(&mut self, loc: Option<[u16; 4]>) {
        if self.pending_loc.is_none() && self.cur_page > 0 {
            self.pending_loc = loc;
        }
    }

    /// Resolve the [`Node::Commented`] annotations collected during the walk
    /// into docling's `comments: [{"$ref": "#/groups/N"}]` key on the annotated
    /// item. It has to run after the whole walk: docx appends its comment
    /// bodies, so the groups they live in are usually allocated *later* than
    /// the paragraphs pointing at them. docling emits the key between `prov`
    /// and `orig`, so insert it in place rather than appending (serde_json runs
    /// with `preserve_order`, i.e. key order is output order).
    fn link_comments(&mut self) {
        let refs: Vec<(String, Vec<Value>)> = std::mem::take(&mut self.pending_comments)
            .into_iter()
            .map(|(item, comments)| {
                let refs = comments
                    .iter()
                    .filter_map(|i| self.comment_groups.get(*i))
                    .map(|r| json!({ "$ref": r }))
                    .collect();
                (item, refs)
            })
            .collect();
        for (item, comment_refs) in refs {
            if comment_refs.is_empty() {
                continue;
            }
            let Some(target) = self.item_mut(&item) else {
                continue;
            };
            let Some(obj) = target.as_object_mut() else {
                continue;
            };
            let tail: Vec<(String, Value)> = obj
                .iter()
                .skip_while(|(k, _)| k.as_str() != "prov")
                .skip(1)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (k, _) in &tail {
                obj.shift_remove(k);
            }
            obj.insert("comments".into(), Value::Array(comment_refs));
            for (k, v) in tail {
                obj.insert(k, v);
            }
        }
    }

    /// The stored JSON object a `#/texts/N`-style self-ref points at.
    fn item_mut(&mut self, self_ref: &str) -> Option<&mut Value> {
        let idx = ref_index(self_ref)?;
        let bucket = if self_ref.starts_with("#/texts/") {
            &mut self.texts
        } else if self_ref.starts_with("#/tables/") {
            &mut self.tables
        } else if self_ref.starts_with("#/pictures/") {
            &mut self.pictures
        } else if self_ref.starts_with("#/groups/") {
            &mut self.groups
        } else {
            return None;
        };
        bucket.get_mut(idx)
    }

    fn add_node(&mut self, node: &Node, parent: &str) -> Option<String> {
        match node {
            Node::Heading { level: 1, text } => {
                Some(self.add_text("title", text, parent, json!({})))
            }
            Node::Heading { level, text } => Some(self.add_text(
                "section_header",
                text,
                parent,
                json!({ "level": level.saturating_sub(1) }),
            )),
            Node::Paragraph { text } => {
                // A whole-paragraph display equation is a formula item (docling
                // wraps it in `$$…$$` and, unlike a text item, never escapes it).
                let t = text.trim();
                match t.strip_prefix("$$").and_then(|s| s.strip_suffix("$$")) {
                    Some(inner) if !inner.is_empty() => Some(self.add_formula(inner, parent)),
                    _ => Some(self.add_text("text", text, parent, json!({}))),
                }
            }
            Node::CheckboxItem { checked, text } => {
                // JSON keeps the task-list form as a plain text item (the
                // `checkbox_selected`/`checkbox_unselected` label is DocLang-only).
                let mark = if *checked { "- [x] " } else { "- [ ] " };
                Some(self.add_text("text", &format!("{mark}{text}"), parent, json!({})))
            }
            Node::Code {
                language,
                text,
                orig,
                ..
            } => Some(self.add_code(text, language.as_deref(), orig.as_deref(), parent)),
            // A CodeFormula-decoded display formula: `text` carries the LaTeX,
            // `orig` the raw glyph extraction (docling's enriched shape).
            Node::Formula {
                latex,
                orig,
                location,
            } => {
                self.adopt_loc(*location);
                Some(self.add_formula_item(latex, orig, parent))
            }
            // docling's notes-layer `comment_section` group holding the
            // comment's text item; the annotated body items point at the group
            // (not the text), which is how replies group under one comment.
            Node::CommentSection { name, text } => {
                let self_ref = format!("#/groups/{}", self.groups.len());
                self.groups.push(Value::Null);
                let child =
                    self.add_text("text", text, &self_ref, json!({ "content_layer": "notes" }));
                self.groups[group_index(&self_ref)] = json!({
                    "self_ref": self_ref,
                    "parent": { "$ref": parent },
                    "children": [{ "$ref": child }],
                    "content_layer": "notes",
                    "name": name,
                    "label": "comment_section",
                });
                self.comment_groups.push(self_ref.clone());
                Some(self_ref)
            }
            // The annotation itself is a cross-reference: emit the item, then
            // remember it so the group refs can be filled in at the end.
            Node::Commented { comments, inner } => {
                let item = self.add_node(inner, parent)?;
                if !comments.is_empty() {
                    self.pending_comments.push((item.clone(), comments.clone()));
                }
                Some(item)
            }
            Node::Table(t) => Some(self.add_table(t, parent)),
            Node::Picture {
                caption,
                caption_href,
                image,
                classification,
            } => Some(self.add_picture(
                caption.as_deref(),
                caption_href.as_deref(),
                image.as_ref(),
                classification.as_deref(),
                parent,
            )),
            // A chart is a picture item in the JSON (its data table is
            // DocLang-only); no image payload.
            Node::Chart {
                caption, location, ..
            } => {
                self.adopt_loc(*location);
                Some(self.add_picture(caption.as_deref(), None, None, None, parent))
            }
            // A DocLang-only node is omitted from the JSON body.
            Node::DoclangOnly(_) => None,
            Node::Group { label, children } => Some(self.add_group(label, children, parent)),
            Node::FieldRegion { items } => Some(self.add_field_region(items, parent)),
            // A rich inline group is a text item over its Markdown text; the
            // structured runs are DocLang-only, so the JSON matches a paragraph.
            Node::InlineGroup { md_text, .. } => {
                Some(self.add_text("text", md_text, parent, json!({})))
            }
            // A plain-text backend dump is a single text item over the file body.
            Node::TextDump(text) => Some(self.add_text("text", text, parent, json!({}))),
            // Furniture is not emitted into the body/JSON (DocLang-only layer).
            Node::Furniture { .. } => None,
            Node::PageFurniture { .. } => None,
            // A location wrapper turns into the wrapped item's `prov` entry —
            // but only on pages the PDF paths described with a PageInfo marker
            // (other geometry-bearing backends, e.g. PPTX shapes, keep their
            // pre-#171 provenance-less JSON until they emit markers too).
            Node::Located { location, inner } => {
                if self.cur_page > 0 {
                    self.pending_loc = Some(*location);
                }
                let r = self.add_node(inner, parent);
                self.pending_loc = None;
                r
            }
            // Page breaks are DocLang-only; docling omits them from the JSON body.
            Node::PageBreak => None,
            // The page marker: record the page's number and size for the
            // `pages` map, and denormalize every following location against it.
            Node::PageInfo {
                page_no,
                width,
                height,
            } => {
                self.cur_page = *page_no;
                self.cur_w = *width as f64;
                self.cur_h = *height as f64;
                if *page_no > 0 {
                    self.pages.push((*page_no, self.cur_w, self.cur_h));
                }
                None
            }
            // Handled by `add_list` in `walk`.
            Node::ListItem { .. } => None,
        }
    }

    /// A form key-value region: `field_regions/N` holds the region, each field is
    /// a `field_items/M` whose children are its `marker` / `field_key` /
    /// `field_value` texts (absent parts are simply omitted).
    fn add_field_region(&mut self, items: &[crate::FieldItem], parent: &str) -> String {
        let self_ref = format!("#/field_regions/{}", self.field_regions.len());
        self.field_regions.push(Value::Null);
        let region_index = self.field_regions.len() - 1;
        let mut item_refs = Vec::new();
        for item in items {
            item_refs.push(json!({ "$ref": self.add_field_item(item, &self_ref) }));
        }
        self.field_regions[region_index] = json!({
            "self_ref": self_ref,
            "parent": { "$ref": parent },
            "children": item_refs,
            "content_layer": "body",
            "label": "field_region",
            "prov": [],
        });
        self_ref
    }

    fn add_field_item(&mut self, item: &crate::FieldItem, parent: &str) -> String {
        let self_ref = format!("#/field_items/{}", self.field_items.len());
        self.field_items.push(Value::Null);
        let item_index = self.field_items.len() - 1;
        let mut child_refs = Vec::new();
        for (label, text) in [
            ("marker", &item.marker),
            ("field_key", &item.key),
            ("field_value", &item.value),
        ] {
            if let Some(text) = text {
                child_refs
                    .push(json!({ "$ref": self.add_text(label, text, &self_ref, json!({})) }));
            }
        }
        self.field_items[item_index] = json!({
            "self_ref": self_ref,
            "parent": { "$ref": parent },
            "children": child_refs,
            "content_layer": "body",
            "label": "field_item",
            "prov": [],
        });
        self_ref
    }

    fn add_text(&mut self, label: &str, text: &str, parent: &str, extra: Value) -> String {
        let self_ref = format!("#/texts/{}", self.texts.len());
        let raw = unescape_text(text);
        let prov = self.take_prov(raw.chars().count());
        let mut item = json!({
            "self_ref": self_ref,
            "parent": { "$ref": parent },
            "children": [],
            "content_layer": "body",
            "label": label,
            "prov": prov,
            "orig": raw,
            "text": raw,
        });
        merge(&mut item, extra);
        self.texts.push(item);
        self_ref
    }

    /// A display-math formula item. `latex` is the raw content (no `$$`); docling
    /// re-wraps it and never escapes it.
    fn add_formula(&mut self, latex: &str, parent: &str) -> String {
        let self_ref = format!("#/texts/{}", self.texts.len());
        let prov = self.take_prov(latex.chars().count());
        self.texts.push(json!({
            "self_ref": self_ref,
            "parent": { "$ref": parent },
            "children": [],
            "content_layer": "body",
            "label": "formula",
            "prov": prov,
            "orig": latex,
            "text": latex,
        }));
        self_ref
    }

    /// A CodeFormula-enriched display formula: `text` is the model's LaTeX
    /// while `orig` keeps the raw glyph extraction (docling's enriched shape;
    /// the plain [`Self::add_formula`] above sets both to the same string).
    fn add_formula_item(&mut self, latex: &str, orig: &str, parent: &str) -> String {
        let self_ref = format!("#/texts/{}", self.texts.len());
        let prov = self.take_prov(latex.chars().count());
        self.texts.push(json!({
            "self_ref": self_ref,
            "parent": { "$ref": parent },
            "children": [],
            "content_layer": "body",
            "label": "formula",
            "prov": prov,
            "orig": orig,
            "text": latex,
        }));
        self_ref
    }

    fn add_code(
        &mut self,
        text: &str,
        language: Option<&str>,
        orig: Option<&str>,
        parent: &str,
    ) -> String {
        let self_ref = format!("#/texts/{}", self.texts.len());
        let raw = unescape_text(text);
        let prov = self.take_prov(raw.chars().count());
        self.texts.push(json!({
            "self_ref": self_ref,
            "parent": { "$ref": parent },
            "children": [],
            "content_layer": "body",
            "label": "code",
            "prov": prov,
            // With code enrichment, `text` is the model's rewrite while `orig`
            // keeps the raw extraction; otherwise both are the same string.
            "orig": orig.map(unescape_text).unwrap_or_else(|| raw.clone()),
            "text": raw,
            "captions": [],
            "references": [],
            "footnotes": [],
            "code_language": code_language(language),
        }));
        self_ref
    }

    /// Build a list group from a run of (possibly multi-level) list items. A
    /// deeper level starts a nested list under the preceding item.
    fn add_list(&mut self, items: &[Node], parent: &str) -> String {
        let self_ref = format!("#/groups/{}", self.groups.len());
        // reserve the slot so nested groups get later indices
        self.groups.push(Value::Null);
        let base = level_of(&items[0]);
        let mut children = Vec::new();
        let mut i = 0;
        while i < items.len() {
            // Empty paragraphs absorbed into the run (blank lines between items)
            // are not list items — skip them.
            if !matches!(items[i], Node::ListItem { .. }) {
                i += 1;
                continue;
            }
            let lvl = level_of(&items[i]);
            if lvl > base {
                // shouldn't happen at the head; skip defensively
                i += 1;
                continue;
            }
            let item_ref = self.add_list_item(&items[i], &self_ref);
            // collect any deeper items that nest under this one
            let mut j = i + 1;
            while j < items.len() && level_of(&items[j]) > base {
                j += 1;
            }
            if j > i + 1 {
                let mut nested = Vec::new();
                self.add_sibling_lists(&items[i + 1..j], &item_ref, &mut nested);
                // the nested list group(s) are children of this item
                if let Some(idx) = ref_index(&item_ref) {
                    self.texts[idx]["children"]
                        .as_array_mut()
                        .unwrap()
                        .extend(nested);
                }
            }
            children.push(json!({ "$ref": item_ref }));
            i = j;
        }
        self.groups[group_index(&self_ref)] = json!({
            "self_ref": self_ref,
            "parent": { "$ref": parent },
            "children": children,
            "content_layer": "body",
            "name": "list",
            "label": "list",
        });
        self_ref
    }

    fn add_list_item(&mut self, node: &Node, parent: &str) -> String {
        let Node::ListItem {
            ordered,
            number,
            text,
            location,
            ..
        } = node
        else {
            unreachable!()
        };
        self.adopt_loc(*location);
        let self_ref = format!("#/texts/{}", self.texts.len());
        let raw = unescape_text(text);
        let prov = self.take_prov(raw.chars().count());
        let marker = if *ordered {
            format!("{number}.")
        } else {
            "-".to_string()
        };
        self.texts.push(json!({
            "self_ref": self_ref,
            "parent": { "$ref": parent },
            "children": [],
            "content_layer": "body",
            "label": "list_item",
            "prov": prov,
            "orig": raw,
            "text": raw,
            "enumerated": ordered,
            "marker": marker,
        }));
        self_ref
    }

    fn add_table(&mut self, t: &Table, parent: &str) -> String {
        let self_ref = format!("#/tables/{}", self.tables.len());
        self.adopt_loc(t.location);
        let prov = self.take_prov(0);
        // The caption is a separate text item the table references (docling's
        // `TableItem.captions`), added before the grid so its box isn't
        // inherited by a later item.
        let mut captions = Vec::new();
        if let Some(cap) = t.caption.as_deref().filter(|c| !c.is_empty()) {
            let cap_ref = self.add_text("caption", cap, &self_ref, json!({}));
            captions.push(json!({ "$ref": cap_ref }));
        }
        let num_rows = t.rows.len();
        let num_cols = t.rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut grid = Vec::with_capacity(num_rows);
        let mut cells = Vec::new();
        if let Some(first_class) = t.cells.as_ref().filter(|c| !c.is_empty()) {
            // First-class cells (#240): serialize the real records — bbox
            // (page points, top-left origin, docling's TableCell shape),
            // span offsets and header roles — and repeat each spanning
            // cell's entry across its covered grid positions, exactly like
            // docling's `TableData.grid`.
            let cell_json = |c: &crate::TableCell| {
                let mut v = json!({
                    "row_span": c.row_span,
                    "col_span": c.col_span,
                    "start_row_offset_idx": c.start_row,
                    "end_row_offset_idx": c.start_row + c.row_span,
                    "start_col_offset_idx": c.start_col,
                    "end_col_offset_idx": c.start_col + c.col_span,
                    "text": unescape_text(&crate::markdown::strip_hard_breaks(&c.text)),
                    "column_header": c.column_header,
                    "row_header": c.row_header,
                    "row_section": c.row_section,
                    "fillable": false,
                });
                if let Some(b) = c.bbox {
                    v["bbox"] = json!({
                        "l": b[0], "t": b[1], "r": b[2], "b": b[3],
                        "coord_origin": "TOPLEFT",
                    });
                }
                v
            };
            let mut by_pos: std::collections::HashMap<(usize, usize), serde_json::Value> =
                std::collections::HashMap::new();
            for c in first_class {
                let v = cell_json(c);
                cells.push(v.clone());
                for r in c.start_row..(c.start_row + c.row_span).min(num_rows) {
                    for k in c.start_col..(c.start_col + c.col_span).min(num_cols) {
                        by_pos.insert((r, k), v.clone());
                    }
                }
            }
            for r in 0..num_rows {
                let mut grid_row = Vec::with_capacity(num_cols);
                for c in 0..num_cols {
                    // A position no cell covers (a hole in the prediction)
                    // falls back to an empty 1×1 entry.
                    grid_row.push(by_pos.get(&(r, c)).cloned().unwrap_or_else(|| {
                        json!({
                            "row_span": 1,
                            "col_span": 1,
                            "start_row_offset_idx": r,
                            "end_row_offset_idx": r + 1,
                            "start_col_offset_idx": c,
                            "end_col_offset_idx": c + 1,
                            "text": "",
                            "column_header": false,
                            "row_header": false,
                            "row_section": false,
                            "fillable": false,
                        })
                    }));
                }
                grid.push(grid_row);
            }
        } else {
            for (r, row) in t.rows.iter().enumerate() {
                let mut grid_row = Vec::with_capacity(num_cols);
                for c in 0..num_cols {
                    // A rich cell's flat text is its Markdown serialization; the
                    // GFM hard-line-break marker (docling-core#721) is
                    // Markdown-only, so JSON sees the raw line breaks.
                    let text = row
                        .get(c)
                        .map(|s| unescape_text(&crate::markdown::strip_hard_breaks(s)))
                        .unwrap_or_default();
                    let cell = json!({
                        "row_span": 1,
                        "col_span": 1,
                        "start_row_offset_idx": r,
                        "end_row_offset_idx": r + 1,
                        "start_col_offset_idx": c,
                        "end_col_offset_idx": c + 1,
                        "text": text,
                        "column_header": r == 0,
                        "row_header": false,
                        "row_section": false,
                        "fillable": false,
                    });
                    grid_row.push(cell.clone());
                    cells.push(cell);
                }
                grid.push(grid_row);
            }
        }
        self.tables.push(json!({
            "self_ref": self_ref,
            "parent": { "$ref": parent },
            "children": [],
            "content_layer": "body",
            "label": "table",
            "prov": prov,
            "captions": captions,
            "references": [],
            "footnotes": [],
            "data": {
                "table_cells": cells,
                "num_rows": num_rows,
                "num_cols": num_cols,
                "grid": grid,
            },
            "annotations": [],
        }));
        self_ref
    }

    fn add_picture(
        &mut self,
        caption: Option<&str>,
        caption_href: Option<&str>,
        image: Option<&crate::PictureImage>,
        classification: Option<&[crate::PictureClass]>,
        parent: &str,
    ) -> String {
        let self_ref = format!("#/pictures/{}", self.pictures.len());
        // Take the picture's own provenance before the caption text is added —
        // the caption is a separate item and must not inherit the crop's box.
        let prov = self.take_prov(0);
        let mut captions = Vec::new();
        if let Some(cap) = caption.filter(|c| !c.is_empty()) {
            // Emit the caption as a text item that the picture references. A
            // wrapping `<a href>`'s link rides as docling's `hyperlink` field
            // on the caption item (#328).
            let extra = match caption_href {
                Some(href) => json!({ "hyperlink": href }),
                None => json!({}),
            };
            let cap_ref = self.add_text("caption", cap, &self_ref, extra);
            captions.push(json!({ "$ref": cap_ref }));
        }
        // DocumentPictureClassifier predictions land twice, exactly like
        // docling 2.x writes them: the (deprecated but still emitted)
        // `classification` annotation and the newer `meta.classification`
        // field (whose per-prediction key order differs — pydantic field
        // order: confidence, created_by, class_name).
        let annotations = match classification {
            Some(classes) => json!([{
                "kind": "classification",
                "provenance": "DocumentPictureClassifier",
                "predicted_classes": classes.iter().map(|c| json!({
                    "class_name": c.class_name,
                    "confidence": c.confidence as f64,
                })).collect::<Vec<_>>(),
            }]),
            None => json!([]),
        };
        // `meta` sits between `content_layer` and `label` in docling's field
        // order (and `preserve_order` keeps ours byte-compatible), so the item
        // is built in one shot per shape rather than patched afterwards.
        let mut item = match classification {
            Some(classes) => json!({
                "self_ref": self_ref,
                "parent": { "$ref": parent },
                "children": [],
                "content_layer": "body",
                "meta": {
                    "classification": {
                        "predictions": classes.iter().map(|c| json!({
                            "confidence": c.confidence as f64,
                            "created_by": "DocumentPictureClassifier",
                            "class_name": c.class_name,
                        })).collect::<Vec<_>>(),
                    },
                },
                "label": "picture",
                "prov": prov,
                "captions": captions,
                "references": [],
                "footnotes": [],
                "annotations": annotations,
            }),
            None => json!({
                "self_ref": self_ref,
                "parent": { "$ref": parent },
                "children": [],
                "content_layer": "body",
                "label": "picture",
                "prov": prov,
                "captions": captions,
                "references": [],
                "footnotes": [],
                "annotations": annotations,
            }),
        };
        // docling stores the extracted image as an `ImageRef` (data URI + size).
        if let Some(img) = image {
            item["image"] = json!({
                "mimetype": img.mimetype,
                "dpi": 72,
                "size": { "width": img.width, "height": img.height },
                "uri": img.data_uri(),
            });
        }
        self.pictures.push(item);
        self_ref
    }

    fn add_group(&mut self, label: &str, nodes: &[Node], parent: &str) -> String {
        let self_ref = format!("#/groups/{}", self.groups.len());
        self.groups.push(Value::Null);
        let children = self.walk_into(nodes, &self_ref);
        let name = if label == "inline" { "group" } else { label };
        self.groups[group_index(&self_ref)] = json!({
            "self_ref": self_ref,
            "parent": { "$ref": parent },
            "children": children,
            "content_layer": "body",
            "name": name,
            "label": label,
        });
        self_ref
    }

    /// Walk a slice of sibling nodes, returning each child's `$ref`; runs of
    /// list items are folded into list groups (one per sibling list).
    fn walk_into(&mut self, nodes: &[Node], parent: &str) -> Vec<Value> {
        let mut children = Vec::new();
        let mut i = 0;
        while i < nodes.len() {
            if matches!(nodes[i], Node::ListItem { .. }) {
                let start = i;
                i += 1;
                loop {
                    match nodes.get(i) {
                        Some(Node::ListItem { .. }) => i += 1,
                        // Absorb an empty paragraph sitting between two list
                        // items (docling keeps the ListGroup contiguous).
                        Some(Node::Paragraph { text })
                            if text.is_empty()
                                && matches!(nodes.get(i + 1), Some(Node::ListItem { .. })) =>
                        {
                            i += 1
                        }
                        _ => break,
                    }
                }
                self.add_sibling_lists(&nodes[start..i], parent, &mut children);
            } else {
                if let Some(r) = self.add_node(&nodes[i], parent) {
                    children.push(json!({ "$ref": r }));
                }
                i += 1;
            }
        }
        children
    }

    /// A run of list items may hold several *sibling* lists; emit one list group
    /// per sibling. The boundary mirrors the Markdown serializer: at the base
    /// level a new list starts on `first_in_list`, a kind flip (`ul`↔`ol`), or an
    /// ordered-number discontinuity.
    fn add_sibling_lists(&mut self, run: &[Node], parent: &str, out: &mut Vec<Value>) {
        let base = level_of(&run[0]);
        let mut seg = 0;
        let mut prev: Option<(bool, u64)> = None;
        // Previous base-level item was a multilevel projection (overlay
        // ordered, flat bullet): its parent-level successor continues the same
        // Word list, so neither heuristic may split there (docling#3902).
        let mut prev_projected = false;
        for k in 0..run.len() {
            let Node::ListItem {
                ordered,
                number,
                first_in_list,
                level,
                dclx,
                ..
            } = &run[k]
            else {
                continue;
            };
            if *level != base {
                continue; // nested item — handled inside add_list
            }
            let eff_ordered = dclx.as_ref().map_or(*ordered, |d| d.ordered);
            if k > seg {
                if let Some((po, pn)) = prev {
                    let same_word_list = prev_projected && eff_ordered;
                    if *first_in_list
                        || (!same_word_list && (po != *ordered || (*ordered && *number != pn + 1)))
                    {
                        out.push(json!({ "$ref": self.add_list(&run[seg..k], parent) }));
                        seg = k;
                    }
                }
            }
            prev = Some((*ordered, *number));
            prev_projected = eff_ordered && !*ordered;
        }
        out.push(json!({ "$ref": self.add_list(&run[seg..], parent) }));
    }
}

fn level_of(node: &Node) -> u8 {
    match node {
        Node::ListItem { level, .. } => *level,
        _ => 0,
    }
}

fn group_index(self_ref: &str) -> usize {
    self_ref.rsplit('/').next().unwrap().parse().unwrap()
}

fn ref_index(self_ref: &str) -> Option<usize> {
    self_ref.rsplit('/').next()?.parse().ok()
}

/// Merge the key/values of `extra` (an object) into `target` (an object).
fn merge(target: &mut Value, extra: Value) {
    if let (Some(t), Some(e)) = (target.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            t.insert(k.clone(), v.clone());
        }
    }
}

/// Reverse [`crate`]'s Markdown text escaping (HTML entities + `\_`).
fn unescape_text(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("\\_", "_")
}

/// 64-bit FNV-1a, a stand-in for docling's `binary_hash` (we lack the source bytes
/// at export time; the value only needs to be a stable u64).
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use crate::{DoclingDocument, ImageMode, Node, PictureImage, Table};
    use serde_json::Value;

    fn doc_with_image() -> DoclingDocument {
        let mut doc = DoclingDocument::new("t");
        doc.push(Node::Picture {
            caption: Some("Fig 1".into()),
            caption_href: None,
            image: Some(PictureImage {
                mimetype: "image/png".into(),
                width: 4,
                height: 2,
                data: b"foobar".to_vec(),
            }),
            classification: None,
        });
        doc
    }

    /// #171: PageInfo markers become the `pages` map, and `Located` wrappers /
    /// node-level locations become per-item `prov` — the 0–511 grid
    /// denormalized against the page into BOTTOMLEFT points. Without markers
    /// (every declarative backend) the JSON stays exactly as before: empty
    /// `pages`, `prov: []` even for located nodes.
    #[test]
    fn page_markers_produce_pages_and_prov() {
        let mut doc = DoclingDocument::new("t");
        doc.push(Node::PageInfo {
            page_no: 1,
            width: 512.0,
            height: 1024.0,
        });
        doc.push(Node::Located {
            location: [128, 64, 256, 128], // quarter/eighth points of the grid
            inner: Box::new(Node::Paragraph {
                text: "hello".into(),
            }),
        });
        doc.push(Node::Table(Table {
            rows: vec![vec!["a".into()]],
            location: Some([0, 0, 512, 512]),
            ..Table::default()
        }));
        let v: Value = serde_json::from_str(&doc.export_to_json()).unwrap();
        assert_eq!(v["pages"]["1"]["page_no"], 1);
        assert_eq!(v["pages"]["1"]["size"]["width"], 512.0);
        assert_eq!(v["pages"]["1"]["size"]["height"], 1024.0);
        // 512-wide page: grid x scales 1:1; 1024-high: grid y doubles, then
        // flips to the BOTTOMLEFT origin (t from grid-top 64 → 1024-128=896).
        let prov = &v["texts"][0]["prov"][0];
        assert_eq!(prov["page_no"], 1);
        assert_eq!(prov["bbox"]["l"], 128.0);
        assert_eq!(prov["bbox"]["t"], 896.0);
        assert_eq!(prov["bbox"]["r"], 256.0);
        assert_eq!(prov["bbox"]["b"], 768.0);
        assert_eq!(prov["bbox"]["coord_origin"], "BOTTOMLEFT");
        assert_eq!(prov["charspan"][1], 5);
        // The table adopts its own location field; charspan is [0, 0].
        let tprov = &v["tables"][0]["prov"][0];
        assert_eq!(tprov["bbox"]["t"], 1024.0);
        assert_eq!(tprov["bbox"]["b"], 0.0);
        assert_eq!(tprov["charspan"][1], 0);

        // No markers → the pre-#171 shape, byte for byte.
        let mut plain = DoclingDocument::new("t");
        plain.push(Node::Located {
            location: [1, 2, 3, 4],
            inner: Box::new(Node::Paragraph { text: "x".into() }),
        });
        let v: Value = serde_json::from_str(&plain.export_to_json()).unwrap();
        assert_eq!(v["pages"], serde_json::json!({}));
        assert_eq!(v["texts"][0]["prov"], serde_json::json!([]));
    }

    #[test]
    fn picture_image_in_markdown_modes_and_json() {
        let doc = doc_with_image();
        // placeholder (default) ignores the image
        assert!(doc.export_to_markdown().contains("<!-- image -->"));
        // embedded → base64 data URI (b"foobar" → "Zm9vYmFy")
        let (md, files) = doc.export_to_markdown_with_images(ImageMode::Embedded, "artifacts");
        assert!(
            md.contains("![Image](data:image/png;base64,Zm9vYmFy)"),
            "got:\n{md}"
        );
        assert!(files.is_empty());
        // referenced → file link + collected bytes
        let (md, files) = doc.export_to_markdown_with_images(ImageMode::Referenced, "artifacts");
        assert!(
            md.contains("![Image](artifacts/image_000000.png)"),
            "got:\n{md}"
        );
        assert_eq!(
            files,
            vec![("artifacts/image_000000.png".to_string(), b"foobar".to_vec())]
        );
        // JSON carries the ImageRef (data URI + size)
        let v: Value = serde_json::from_str(&doc.export_to_json()).unwrap();
        assert_eq!(v["pictures"][0]["image"]["mimetype"], "image/png");
        assert_eq!(v["pictures"][0]["image"]["size"]["width"], 4);
        assert_eq!(
            v["pictures"][0]["image"]["uri"],
            "data:image/png;base64,Zm9vYmFy"
        );
    }

    #[test]
    fn exports_docling_schema() {
        let mut doc = DoclingDocument::new("t");
        doc.push(Node::Heading {
            level: 1,
            text: "Title".into(),
        });
        doc.push(Node::Heading {
            level: 2,
            text: "Sec".into(),
        });
        doc.push(Node::Paragraph {
            text: "Body &amp; more".into(),
        }); // markdown-escaped
        doc.push(Node::ListItem {
            ordered: false,
            number: 0,
            first_in_list: true,
            text: "one".into(),
            level: 0,
            marker: None,
            location: None,
            dclx: None,
            href: None,
            layer: None,
        });
        doc.push(Node::ListItem {
            ordered: false,
            number: 0,
            first_in_list: false,
            text: "two".into(),
            level: 0,
            marker: None,
            location: None,
            dclx: None,
            href: None,
            layer: None,
        });
        doc.push(Node::Table(Table {
            rows: vec![vec!["A".into(), "B".into()]],
            location: None,
            structure: None,
            cell_blocks: None,
            cells: None,
            caption: None,
        }));

        let v: Value = serde_json::from_str(&doc.export_to_json()).unwrap();
        assert_eq!(v["schema_name"], "DoclingDocument");
        assert_eq!(v["version"], "1.10.0");
        assert_eq!(v["texts"][0]["label"], "title");
        assert_eq!(v["texts"][1]["label"], "section_header");
        assert_eq!(v["texts"][1]["level"], 1); // heading level 2 → docling level 1
        assert_eq!(v["texts"][2]["text"], "Body & more"); // un-escaped for the wire format
                                                          // consecutive list items fold into one list group, parented to it
        assert_eq!(v["groups"][0]["label"], "list");
        assert_eq!(v["groups"][0]["children"].as_array().unwrap().len(), 2);
        assert_eq!(v["texts"][3]["parent"]["$ref"], "#/groups/0");
        assert_eq!(v["texts"][3]["marker"], "-");
        // table grid + header flag
        assert_eq!(v["tables"][0]["data"]["num_cols"], 2);
        assert_eq!(v["tables"][0]["data"]["grid"][0][0]["column_header"], true);
    }
    /// docx reviewer comments: a `comment_section` group on the notes layer
    /// holding the note text, and a `comments` back-ref on the annotated item —
    /// keyed between `prov` and `orig`, the slot docling emits it in.
    #[test]
    fn comment_sections_link_back_to_their_items() {
        let doc = DoclingDocument {
            name: "c".into(),
            nodes: vec![
                Node::Commented {
                    comments: vec![0],
                    inner: Box::new(Node::Paragraph {
                        text: "annotated".into(),
                    }),
                },
                Node::Paragraph {
                    text: "plain".into(),
                },
                Node::CommentSection {
                    name: "comment-7".into(),
                    text: "[time: t]: note".into(),
                },
            ],
            ..DoclingDocument::new("c")
        };
        let v = crate::json::to_json(&doc);
        // The group is the comment section; its only child is the notes text.
        assert_eq!(v["groups"][0]["label"], "comment_section");
        assert_eq!(v["groups"][0]["name"], "comment-7");
        assert_eq!(v["groups"][0]["content_layer"], "notes");
        assert_eq!(v["groups"][0]["children"][0]["$ref"], "#/texts/2");
        assert_eq!(v["texts"][2]["content_layer"], "notes");
        // The annotated item points back at the group; the plain one has no key.
        assert_eq!(v["texts"][0]["comments"][0]["$ref"], "#/groups/0");
        assert!(v["texts"][1].get("comments").is_none());
        // docling's key order: … prov, comments, orig, text.
        let keys: Vec<&str> = v["texts"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            &keys[keys.len() - 4..],
            &["prov", "comments", "orig", "text"]
        );
    }
}
