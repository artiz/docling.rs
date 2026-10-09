//! docling's **item tree**, for a backend that knows the exact shape upstream
//! gives a document and wants the JSON export to reproduce it.
//!
//! [`DoclingDocument::nodes`](crate::DoclingDocument::nodes) is a flat,
//! reading-order stream tuned for Markdown / DocLang / LaTeX; the JSON export
//! rebuilds docling's parent/child structure from it with generic rules (runs
//! of list items become list groups, a heading is a flat sibling of the text
//! that follows it). Upstream's backends do not all agree on that structure:
//! the HTML backend nests everything after a heading *under* the heading,
//! splits a paragraph of mixed formatting into an `inline` group of one text
//! item per formatting run, parents a rich table cell's content to a group
//! under the table, keeps site chrome on the `furniture` layer… and numbers
//! every item in the order it *creates* them. A backend that ports those
//! rules call-for-call (HTML's `html_tree.rs`, DOCX's `docx_tree.rs`) records
//! the result here — an arena of items in
//! creation order, each with its parent and children — and the JSON export
//! ([`DoclingDocument::export_to_json`](crate::DoclingDocument::export_to_json))
//! serializes this tree instead of deriving one from the nodes. Every other
//! serializer keeps reading the flat nodes, so their output is unaffected.

use crate::{ContentLayer, FieldItem, PictureImage, Script, Table};

/// docling-core's `Formatting`: the inline styles an item carries in JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Formatting {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub script: Script,
}

/// A `list_item`'s docling fields.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListMeta {
    pub enumerated: bool,
    /// docling's `marker` — the HTML backend writes `""` unless an ordered
    /// list carries an explicit `start`, then `"{n}."`.
    pub marker: String,
}

/// What an item in the tree is. Mirrors the docling-core item classes the
/// JSON `texts` / `groups` / `tables` / `pictures` / `field_regions` buckets
/// hold.
#[derive(Debug, Clone, PartialEq)]
pub enum TreeKind {
    /// A `TextItem` / `TitleItem` / `SectionHeaderItem` / `ListItem`, told
    /// apart by `label` (`text`, `title`, `section_header`, `list_item`,
    /// `caption`, `checkbox_selected`, `checkbox_unselected`, …).
    Text {
        label: String,
        text: String,
        /// docling's `orig` when it differs from `text` (the heading text
        /// before unicode cleanup, say); `None` = same as `text`.
        orig: Option<String>,
        formatting: Option<Formatting>,
        hyperlink: Option<String>,
        /// `section_header` only: docling's heading level.
        level: Option<u8>,
        /// `list_item` only.
        list: Option<ListMeta>,
    },
    /// A `CodeItem`.
    Code {
        text: String,
        orig: Option<String>,
        /// The language hint (a highlighter class token such as `python`),
        /// mapped onto docling's `CodeLanguageLabel` at export; `None` →
        /// `unknown`.
        language: Option<String>,
        formatting: Option<Formatting>,
        hyperlink: Option<String>,
    },
    /// A `GroupItem`: `label` is docling's `GroupLabel` value (`inline`,
    /// `list`, `section`, `unspecified`, …), `name` its name (`group`, `list`,
    /// `ordered list`, `header-2`, `rich_cell_group_1_0_3`, …).
    Group { label: String, name: String },
    /// A `TableItem`. `rich_cells` marks the cells docling serialized as a
    /// `RichTableCell`: `(row, col)` grid anchor → the group item (a child of
    /// the table) that holds the cell's content. `captions` are caption text
    /// items in the tree.
    Table {
        table: Table,
        rich_cells: Vec<(usize, usize, usize)>,
        captions: Vec<usize>,
    },
    /// A `PictureItem`, its caption text items and optional payload.
    /// `classification` is a `PictureClassificationLabel` value written as
    /// the picture's `meta.classification` (an HTML `<stamp>` / `<signature>`).
    Picture {
        captions: Vec<usize>,
        image: Option<PictureImage>,
        classification: Option<String>,
        /// The picture-OCR enrichment's text (#645), serialized like the flat
        /// node's: `meta.description` + the `description` annotation.
        description: Option<crate::PictureDescription>,
        /// The prediction's `confidence`, when the backend writes one: the
        /// DocLang deserializer stamps `1.0`; the office and HTML backends
        /// leave it out (`None`).
        confidence: Option<f64>,
        /// A native chart's data grid (docling's `meta.tabular_chart.chart_data`,
        /// the series reconstructed as a `TableData`), for a DOCX chart drawing.
        chart: Option<Table>,
        /// The `ImageRef.dpi` docling writes for `image` when the backend
        /// read one from the file (python-pptx's `Image.dpi`: PIL's `dpi`
        /// info, rounded, 72 when absent or out of 1–2048); `None` → 72,
        /// which is what upstream's other office backends pass.
        dpi: Option<u32>,
    },
    /// A form key-value region (`field_regions` / `field_items`).
    FieldRegion { items: Vec<FieldItem> },
    /// A `KeyValueItem` (`key_value_items`): docling's `GraphData` of key and
    /// value cells and their links, written verbatim.
    KeyValueGraph {
        cells: Vec<crate::GraphCell>,
        links: Vec<crate::GraphLink>,
    },
}

/// docling's `ProvenanceItem` for a tree item, written verbatim: the
/// backend's own geometry in the page's units — a PPTX shape's EMU box,
/// whose `pages` entry is the slide size in EMU — rather than the 0–511
/// DocLang grid the flat [`Node::Located`](crate::Node::Located) carries
/// (which cannot round-trip those integers).
#[derive(Debug, Clone, PartialEq)]
pub struct TreeProv {
    /// 1-based page (slide) number.
    pub page_no: usize,
    /// `[l, t, r, b]`, exactly as docling computed them.
    pub bbox: [f64; 4],
    /// docling's `coord_origin` tag. The office backends tag their
    /// top-left-based boxes `TOPLEFT` (the PPTX backend since docling#4294 —
    /// it used to tag them `BOTTOMLEFT`, which read the tuple as `(l, b, r,
    /// t)` and swapped the vertical edges); a backend that really works in a
    /// bottom-left space sets this.
    pub bottom_left: bool,
    /// `[0, len(text)]` in characters for a text item, `[0, 0]` for a table
    /// or picture.
    pub charspan: [usize; 2],
}

/// docling's `TrackSource` — where in a time-based track (a WebVTT cue) a
/// text item came from. Written as the item's `source: [{"kind": "track", …}]`.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeTrack {
    /// The cue's start offset in seconds (docling's `WebVTTTimestamp.seconds`:
    /// `h*3600 + m*60 + s + millis/1000.0`, so the float is bit-identical).
    pub start_time: f64,
    /// The cue's end offset in seconds.
    pub end_time: f64,
    /// The cue identifier line, when the cue has one.
    pub identifier: Option<String>,
    /// The `<v …>` voice annotation the text sits in, when any.
    pub voice: Option<String>,
}

/// One item of an [`ItemTree`].
#[derive(Debug, Clone, PartialEq)]
pub struct TreeItem {
    /// The parent item's index; `None` = the document body.
    pub parent: Option<usize>,
    /// Child item indices, in docling's `children` order.
    pub children: Vec<usize>,
    /// The content layer; `None` = `body`.
    pub layer: Option<ContentLayer>,
    pub kind: TreeKind,
    /// The item's `prov` entry, when the backend has page geometry for it
    /// (`None` → `prov: []`, what the HTML and DOCX backends write).
    pub prov: Option<TreeProv>,
    /// docling's `DocItem.comments`: the `comment_section` groups (or note
    /// text items) annotating this item, as item indices — written after
    /// `prov` when non-empty.
    pub comments: Vec<usize>,
    /// docling's `DocItem.source`: the track segment a text item was taken
    /// from (WebVTT cues) — written after `prov` when set.
    pub source: Option<TreeTrack>,
    /// Removed by [`ItemTree::delete`] (docling's `delete_items`): the slot
    /// stays so every other index keeps its meaning, but the item is not
    /// numbered or written.
    pub deleted: bool,
    /// Footnotes / endnotes referenced from inside this text item (#538): the
    /// note call's position (chars into the item's text) and the note's text.
    /// docling keeps notes as unlinked furniture `footnote` items, so the
    /// JSON never shows this; the Pandoc AST writes each as a `Note` there.
    pub notes: Vec<TreeNote>,
    /// This furniture `footnote` item is the body of a note some text item
    /// calls (it travels in that item's [`Self::notes`]); the Pandoc AST then
    /// leaves it out as a standalone block.
    pub note_body: bool,
}

/// A note call inside a text item ([`TreeItem::notes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNote {
    /// The call's position, in chars into the item's `text`.
    pub offset: usize,
    /// The note's plain text.
    pub text: String,
}

impl ItemTree {
    /// Where each note call of one paragraph lands (#538): `full_text` is the
    /// paragraph's text, `offsets` the calls' positions in it (chars), and
    /// the paragraph's items are those created from `first_new` on. Each text
    /// item is a trimmed slice of the paragraph (a formatting run, a link, or
    /// the whole heading / list item), matched in order; a call inside an
    /// item lands there, one between items ends the earlier (or starts the
    /// first). When no item matches, the call ends the paragraph's last text
    /// item; with no text item at all it is `None`.
    pub fn place_note_calls(
        &self,
        first_new: usize,
        full_text: &str,
        offsets: &[usize],
    ) -> Vec<Option<(usize, usize)>> {
        let texts: Vec<(usize, &str)> = (first_new..self.items.len())
            .filter(|&i| !self.items[i].deleted)
            .filter_map(|i| match &self.items[i].kind {
                TreeKind::Text { text, .. } | TreeKind::Code { text, .. } => {
                    Some((i, text.as_str()))
                }
                _ => None,
            })
            .collect();
        // Each found item's (id, start, end) in chars of `full_text`.
        let mut spans: Vec<(usize, usize, usize)> = Vec::new();
        let mut cursor = 0usize; // bytes
        for &(item, text) in &texts {
            if text.is_empty() {
                continue;
            }
            if let Some(pos) = full_text[cursor..].find(text) {
                let start_b = cursor + pos;
                let start = full_text[..start_b].chars().count();
                spans.push((item, start, start + text.chars().count()));
                cursor = start_b + text.len();
            }
        }
        offsets
            .iter()
            .map(|&offset| {
                spans
                    .iter()
                    .find(|&&(_, start, end)| offset >= start && offset <= end)
                    .map(|&(item, start, _)| (item, offset - start))
                    .or_else(
                        || match spans.iter().rev().find(|&&(_, _, end)| end <= offset) {
                            Some(&(item, start, end)) => Some((item, end - start)),
                            None => spans.first().map(|&(item, _, _)| (item, 0)),
                        },
                    )
                    .or_else(|| {
                        texts
                            .last()
                            .map(|&(item, text)| (item, text.chars().count()))
                    })
            })
            .collect()
    }
}

/// docling's item tree in creation order (see the [module docs](self)).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ItemTree {
    /// Every item, indexed by creation order — which is how docling numbers
    /// `#/texts/N`, `#/groups/N`, … within each bucket.
    pub items: Vec<TreeItem>,
    /// The body's `children`, as item indices.
    pub body: Vec<usize>,
}

impl ItemTree {
    /// Append an item under `parent` (`None` = body) on `layer`, registering
    /// it as its parent's last child — docling's `add_*` calls do exactly that.
    pub fn add(
        &mut self,
        parent: Option<usize>,
        layer: Option<ContentLayer>,
        kind: TreeKind,
    ) -> usize {
        let id = self.items.len();
        self.items.push(TreeItem {
            parent,
            children: Vec::new(),
            layer,
            kind,
            prov: None,
            comments: Vec::new(),
            source: None,
            deleted: false,
            notes: Vec::new(),
            note_body: false,
        });
        match parent {
            Some(p) => self.items[p].children.push(id),
            None => self.body.push(id),
        }
        id
    }

    /// [`add`](Self::add) with the item's provenance — docling's
    /// `add_text(…, prov=prov)`.
    pub fn add_with_prov(
        &mut self,
        parent: Option<usize>,
        layer: Option<ContentLayer>,
        kind: TreeKind,
        prov: TreeProv,
    ) -> usize {
        let id = self.add(parent, layer, kind);
        self.items[id].prov = Some(prov);
        id
    }

    /// Append every item of `other` after this tree's, renumbering its
    /// indices (parents, children, comments, table/picture caption and
    /// rich-cell refs) and adding its body children to this body — so a
    /// backend can build independent fragments in parallel (one per PPTX
    /// slide) and still hand the export one tree in creation order, exactly
    /// as if it had been built sequentially.
    pub fn append(&mut self, other: ItemTree) {
        let off = self.items.len();
        let shift = |i: usize| i + off;
        for mut item in other.items {
            item.parent = item.parent.map(shift);
            for c in item.children.iter_mut().chain(item.comments.iter_mut()) {
                *c = shift(*c);
            }
            match &mut item.kind {
                TreeKind::Table {
                    rich_cells,
                    captions,
                    ..
                } => {
                    for (_, _, g) in rich_cells.iter_mut() {
                        *g = shift(*g);
                    }
                    for c in captions.iter_mut() {
                        *c = shift(*c);
                    }
                }
                TreeKind::Picture { captions, .. } => {
                    for c in captions.iter_mut() {
                        *c = shift(*c);
                    }
                }
                _ => {}
            }
            self.items.push(item);
        }
        self.body.extend(other.body.into_iter().map(shift));
    }

    /// Move `id` under `new_parent`, dropping it from its current parent's
    /// children and appending it to the new one's — docling's
    /// `group_cell_elements` re-parenting of a rich cell's items.
    pub fn reparent(&mut self, id: usize, new_parent: Option<usize>) {
        let old = self.items[id].parent;
        let siblings = match old {
            Some(p) => &mut self.items[p].children,
            None => &mut self.body,
        };
        siblings.retain(|&c| c != id);
        self.items[id].parent = new_parent;
        match new_parent {
            Some(p) => self.items[p].children.push(id),
            None => self.body.push(id),
        }
    }

    /// Re-number the items in traversal order — a pre-order walk of the body
    /// through every layer, which is how docling's
    /// `DoclingDocument.concatenate` (and `_normalize_references`) re-creates
    /// a document's items: a group created after the content it was later
    /// wrapped around comes before that content afterwards. Items the walk
    /// does not reach (deleted ones) are dropped. Returns each old id's new
    /// id.
    pub fn renumber_in_traversal_order(&mut self) -> Vec<Option<usize>> {
        let mut order = Vec::with_capacity(self.items.len());
        let mut stack: Vec<usize> = self.body.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            if self.items[id].deleted {
                continue;
            }
            order.push(id);
            stack.extend(self.items[id].children.iter().rev());
        }
        let mut new_of: Vec<Option<usize>> = vec![None; self.items.len()];
        for (new, &old) in order.iter().enumerate() {
            new_of[old] = Some(new);
        }
        let remap = |ids: &mut Vec<usize>| {
            *ids = ids.iter().filter_map(|&i| new_of[i]).collect();
        };
        let mut old_items: Vec<Option<TreeItem>> = std::mem::take(&mut self.items)
            .into_iter()
            .map(Some)
            .collect();
        for &old in &order {
            let mut item = old_items[old].take().expect("each item visited once");
            item.parent = item.parent.and_then(|p| new_of[p]);
            remap(&mut item.children);
            remap(&mut item.comments);
            match &mut item.kind {
                TreeKind::Table {
                    rich_cells,
                    captions,
                    ..
                } => {
                    rich_cells.retain_mut(|(_, _, g)| match new_of[*g] {
                        Some(n) => {
                            *g = n;
                            true
                        }
                        None => false,
                    });
                    remap(captions);
                }
                TreeKind::Picture { captions, .. } => remap(captions),
                _ => {}
            }
            self.items.push(item);
        }
        remap(&mut self.body);
        new_of
    }

    /// Remove `id` from the tree — docling's `delete_items`, which the DOCX
    /// backend uses to drop the empty text item a blank spacer paragraph left
    /// between two items of a resumed list. The item leaves its parent's
    /// children and is neither numbered nor written; its slot stays so the
    /// indices held elsewhere stay valid.
    pub fn delete(&mut self, id: usize) {
        match self.items[id].parent {
            Some(p) => self.items[p].children.retain(|&c| c != id),
            None => self.body.retain(|&c| c != id),
        }
        self.items[id].deleted = true;
    }

    /// docling-core's `DoclingDocument.validate_misplaced_list_items` (a
    /// model validator, so it runs whenever docling-core serializes or loads
    /// a document): every `list_item` whose parent is not a `list` group is
    /// re-homed into a new one. A pre-order walk of the body (groups
    /// included) collects them; consecutive misplaced items directly on the
    /// body share one group, any other misplaced item gets its own. Working
    /// from the last run back, each run gets a `ListGroup` (name `group`)
    /// inserted where its first item stood, the items are deleted and
    /// re-added under the group as fresh items, so they move to the end of
    /// the text numbering (#527: the DOCX backend leaves such items in rich
    /// table cells). A no-op for a well-formed tree.
    ///
    /// One deliberate divergence (#586): docling-core deletes each item *with
    /// its children* and re-adds it from its text alone, so a mixed-format
    /// item — an empty `list_item` over an `inline` group of text runs —
    /// comes back empty and its text is gone from the JSON (and from the
    /// Markdown/HTML docling serializes after that validator ran; the JSON
    /// docling saves *before* exporting still has it). Here the children
    /// follow the item into the group: the structure docling-core requires,
    /// the content the document had.
    pub fn wrap_misplaced_list_items(&mut self) {
        let is_list_item = |t: &Self, id: usize| matches!(&t.items[id].kind, TreeKind::Text { label, .. } if label == "list_item");
        let in_list_group = |t: &Self, id: usize| {
            t.items[id].parent.is_some_and(
                |p| matches!(&t.items[p].kind, TreeKind::Group { label, .. } if label == "list"),
            )
        };
        let mut runs: Vec<Vec<usize>> = Vec::new();
        // `None` = the body itself, which the walk yields first.
        let mut prev: Option<usize> = None;
        let mut stack: Vec<usize> = self.body.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            if self.items[id].deleted {
                continue;
            }
            if is_list_item(self, id) && !in_list_group(self, id) {
                let continues =
                    prev.is_some_and(|p| is_list_item(self, p) && self.items[p].parent.is_none());
                match runs.last_mut() {
                    Some(run) if continues => run.push(id),
                    _ => runs.push(vec![id]),
                }
            }
            prev = Some(id);
            stack.extend(self.items[id].children.iter().rev().copied());
        }
        for run in runs.into_iter().rev() {
            let parent = self.items[run[0]].parent;
            let group = self.add(
                parent,
                None,
                TreeKind::Group {
                    label: "list".into(),
                    name: "group".into(),
                },
            );
            let siblings = match parent {
                Some(p) => &mut self.items[p].children,
                None => &mut self.body,
            };
            siblings.pop();
            let at = siblings
                .iter()
                .position(|&c| c == run[0])
                .unwrap_or(siblings.len());
            siblings.insert(at, group);
            for &li in &run {
                // Not `delete_subtree`: the children move to the copy (#586).
                self.delete(li);
            }
            // `add_list_item` keeps the text, marker, formatting, hyperlink
            // and first provenance — not comments or a source. The children
            // (the inline group of a mixed-format item) are carried over.
            for &li in &run {
                let children = std::mem::take(&mut self.items[li].children);
                let copy = TreeItem {
                    parent: Some(group),
                    children: children.clone(),
                    comments: Vec::new(),
                    source: None,
                    deleted: false,
                    ..self.items[li].clone()
                };
                let id = self.items.len();
                self.items.push(copy);
                for c in children {
                    self.items[c].parent = Some(id);
                }
                self.items[group].children.push(id);
            }
        }
    }

    /// The last live text-bucket item (docling's `doc.texts[-1]`).
    pub fn last_text(&self) -> Option<usize> {
        self.items.iter().rposition(|it| {
            !it.deleted && matches!(it.kind, TreeKind::Text { .. } | TreeKind::Code { .. })
        })
    }

    /// How many items of a bucket precede `id` — its `#/{bucket}/N` index.
    pub fn bucket_index(&self, id: usize) -> usize {
        let same = |k: &TreeKind| {
            std::mem::discriminant(k) == std::mem::discriminant(&self.items[id].kind)
                || matches!(
                    (k, &self.items[id].kind),
                    (TreeKind::Text { .. }, TreeKind::Code { .. })
                        | (TreeKind::Code { .. }, TreeKind::Text { .. })
                )
        };
        self.items[..id]
            .iter()
            .filter(|it| !it.deleted && same(&it.kind))
            .count()
    }

    /// The number of tables created so far (docling's `len(doc.tables)`).
    pub fn table_count(&self) -> usize {
        self.items
            .iter()
            .filter(|it| !it.deleted && matches!(it.kind, TreeKind::Table { .. }))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> TreeKind {
        TreeKind::Text {
            label: "text".into(),
            text: t.into(),
            orig: None,
            formatting: None,
            hyperlink: None,
            level: None,
            list: None,
        }
    }

    /// `add` registers the item as its parent's (or the body's) last child;
    /// `reparent` moves it — a rich cell's items leave the heading they were
    /// created under for the table's group.
    #[test]
    fn add_and_reparent_keep_docling_children_order() {
        let mut t = ItemTree::default();
        let title = t.add(None, None, text("Title"));
        let a = t.add(Some(title), None, text("a"));
        let b = t.add(Some(title), None, text("b"));
        let table = t.add(
            Some(title),
            None,
            TreeKind::Table {
                table: Table::default(),
                rich_cells: Vec::new(),
                captions: Vec::new(),
            },
        );
        let group = t.add(
            Some(table),
            None,
            TreeKind::Group {
                label: "unspecified".into(),
                name: "rich_cell_group_1_0_0".into(),
            },
        );
        assert_eq!(t.body, vec![title]);
        assert_eq!(t.items[title].children, vec![a, b, table]);
        t.reparent(a, Some(group));
        assert_eq!(t.items[title].children, vec![b, table]);
        assert_eq!(t.items[group].children, vec![a]);
        assert_eq!(t.items[a].parent, Some(group));
        assert_eq!(t.table_count(), 1);
        // Text and code share the `texts` bucket.
        let code = t.add(
            None,
            None,
            TreeKind::Code {
                text: "x".into(),
                orig: None,
                language: None,
                formatting: None,
                hyperlink: None,
            },
        );
        assert_eq!(t.bucket_index(code), 3, "title, a, b precede it in `texts`");
        assert_eq!(t.bucket_index(group), 0);
        assert_eq!(t.body, vec![title, code]);
    }

    /// `append` renumbers a fragment built on its own (a slide converted in
    /// parallel) so the merged tree reads as if built in one pass: parents,
    /// children, comment back-refs and caption refs all shift together.
    #[test]
    fn append_renumbers_a_fragment_into_creation_order() {
        let mut whole = ItemTree::default();
        let slide0 = whole.add(
            None,
            None,
            TreeKind::Group {
                label: "chapter".into(),
                name: "slide-0".into(),
            },
        );
        whole.add(Some(slide0), None, text("first"));

        let mut frag = ItemTree::default();
        let slide1 = frag.add(
            None,
            None,
            TreeKind::Group {
                label: "chapter".into(),
                name: "slide-1".into(),
            },
        );
        let cap = frag.add_with_prov(
            Some(slide1),
            None,
            TreeKind::Text {
                label: "caption".into(),
                text: "Title".into(),
                orig: None,
                formatting: None,
                hyperlink: None,
                level: None,
                list: None,
            },
            TreeProv {
                page_no: 2,
                bbox: [1.0, 2.0, 3.0, 4.0],
                bottom_left: true,
                charspan: [0, 5],
            },
        );
        let pic = frag.add(
            Some(slide1),
            None,
            TreeKind::Picture {
                captions: vec![cap],
                image: None,
                classification: Some("bar_chart".into()),
                description: None,
                confidence: None,
                chart: None,
                dpi: None,
            },
        );
        let note = frag.add(
            None,
            Some(ContentLayer::Notes),
            TreeKind::Group {
                label: "comment_section".into(),
                name: "comment-slide2-1".into(),
            },
        );
        frag.items[pic].comments.push(note);

        whole.append(frag);
        assert_eq!(whole.body, vec![slide0, 2, 5]);
        assert_eq!(whole.items[2].children, vec![3, 4]);
        assert_eq!(whole.items[3].parent, Some(2));
        assert_eq!(whole.items[3].prov.as_ref().map(|p| p.page_no), Some(2));
        assert!(
            matches!(&whole.items[4].kind, TreeKind::Picture { captions, .. } if captions == &[3])
        );
        assert_eq!(whole.items[4].comments, vec![5]);
        assert_eq!(whole.items[5].parent, None);
        assert_eq!(
            whole.bucket_index(4),
            0,
            "the fragment's picture is #/pictures/0"
        );
        assert_eq!(
            whole.bucket_index(5),
            2,
            "slide-0, slide-1 precede it in `groups`"
        );
    }

    /// `renumber_in_traversal_order`: docling's `concatenate` re-creates the
    /// items as a pre-order walk meets them, so a group created after the
    /// content it was wrapped around (a rich cell's group) comes first, and a
    /// deleted item disappears; every cross-reference follows.
    #[test]
    fn renumbering_follows_the_traversal() {
        let mut t = ItemTree::default();
        let a = t.add(None, None, text("a"));
        let table = t.add(
            None,
            None,
            TreeKind::Table {
                table: Table::default(),
                rich_cells: Vec::new(),
                captions: Vec::new(),
            },
        );
        let cell_text = t.add(None, None, text("cell"));
        let group = t.add(
            Some(table),
            None,
            TreeKind::Group {
                label: "unspecified".into(),
                name: "rich_cell_group_1_0_0".into(),
            },
        );
        t.reparent(cell_text, Some(group));
        if let TreeKind::Table { rich_cells, .. } = &mut t.items[table].kind {
            rich_cells.push((0, 0, group));
        }
        let gone = t.add(None, None, text("gone"));
        t.delete(gone);
        let z = t.add(None, None, text("z"));

        let new_of = t.renumber_in_traversal_order();
        assert_eq!(
            new_of,
            vec![Some(0), Some(1), Some(3), Some(2), None, Some(4)]
        );
        assert_eq!(t.items.len(), 5);
        assert_eq!(t.body, vec![0, 1, 4]);
        assert_eq!(t.items[1].children, vec![2], "the group follows its table");
        assert_eq!(t.items[2].parent, Some(1));
        assert_eq!(
            t.items[3].parent,
            Some(2),
            "the cell text follows its group"
        );
        assert!(matches!(&t.items[3].kind, TreeKind::Text { text, .. } if text == "cell"));
        assert!(
            matches!(&t.items[1].kind, TreeKind::Table { rich_cells, .. } if rich_cells == &[(0, 0, 2)])
        );
        let _ = (a, z);
    }

    /// #586: a misplaced `list_item` keeps its children when it is re-homed.
    /// docling-core's `validate_misplaced_list_items` re-adds the item from
    /// its text alone, so a mixed-format item (empty `list_item` over an
    /// `inline` group of runs) lost every run — a DOCX table cell whose list
    /// paragraph followed a `numId 0` spacer came out as an empty bullet in
    /// the JSON and the HTML. The group goes where the item stood, the copy
    /// is numbered last, the inline group and its texts move under it.
    #[test]
    fn wrapping_a_misplaced_list_item_keeps_its_runs() {
        let mut t = ItemTree::default();
        let cell = t.add(
            None,
            None,
            TreeKind::Group {
                label: "unspecified".into(),
                name: "rich_cell_group_1_0_1".into(),
            },
        );
        let before = t.add(Some(cell), None, text("before"));
        let item = t.add(
            Some(cell),
            None,
            TreeKind::Text {
                label: "list_item".into(),
                text: String::new(),
                orig: None,
                formatting: None,
                hyperlink: None,
                level: None,
                list: Some(ListMeta {
                    enumerated: false,
                    marker: String::new(),
                }),
            },
        );
        let inline = t.add(
            Some(item),
            None,
            TreeKind::Group {
                label: "inline".into(),
                name: "group".into(),
            },
        );
        let run_a = t.add(Some(inline), None, text("Second item text"));
        let run_b = t.add(Some(inline), None, text("[Optional]"));
        let after = t.add(Some(cell), None, text("after"));

        t.wrap_misplaced_list_items();

        let group = t.items[cell].children[1];
        assert_eq!(t.items[cell].children, vec![before, group, after]);
        assert!(
            matches!(&t.items[group].kind, TreeKind::Group { label, name } if label == "list" && name == "group")
        );
        assert!(t.items[item].deleted, "the original item is deleted");
        let copy = t.items[group].children[0];
        assert_ne!(copy, item);
        assert!(
            copy > after,
            "the copy is numbered after every existing item"
        );
        assert_eq!(t.items[copy].children, vec![inline]);
        assert_eq!(t.items[inline].parent, Some(copy));
        assert_eq!(t.items[inline].children, vec![run_a, run_b]);
        for id in [inline, run_a, run_b] {
            assert!(!t.items[id].deleted, "item {id} must survive the re-homing");
        }
    }
}
