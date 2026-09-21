//! AsciiDoc backend.
//!
//! A line-oriented port of docling's `AsciiDocBackend._parse`: titles (`= `),
//! section headers (`== `…), bullet/numbered lists (with `+` continuation and
//! literal-block/image children), `....`-delimited literal blocks, bare and
//! `|===`-delimited tables (with cell-format-specifier stripping), images and
//! captions, and multi-line paragraphs.
//!
//! Alongside the flat nodes the parser builds docling's item tree for the
//! JSON export, mirroring `_parse`'s `parents` / `indents` bookkeeping: the
//! title is the root the sections hang off, a heading is parented to the
//! nearest present ancestor level, every list opens a `list` group at the
//! next level (a deeper indent nests another group, a dedent pops levels
//! while an outer group exists), items are children of the innermost group,
//! a `+`-continued picture or literal block is a child of the open list item,
//! block titles become body-level `caption` items that tables / pictures /
//! code reference — or a bold paragraph when they precede a list.

use docling_core::tree::{Formatting, ItemTree, ListMeta, TreeKind};
use docling_core::{DoclingDocument, Node, Table, TableCell};

use crate::backend::images::{FsImageResolver, ImageResolver, NoFetch};
use crate::backend::markdown::escape_text;
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;

/// AsciiDoc cell specifier, e.g. `2*`, `^`, `.^`, `h` — the run that can be
/// glued before a `|` in a table line. Mirrors docling's `_CELL_SPEC`.
/// AsciiDoc writes the span as `[colspan][.rowspan]` followed by `+` or `*`,
/// and either number may be omitted, so `.2+` is a rowspan on its own
/// (docling#4290, 2.129); requiring one of the two keeps a bare `+` from
/// matching.
const CELL_SPEC: &str = r"(?:(?:\d+(?:\.\d+)?|\.\d+)[*+])*[<^>]?(?:\.[<^>])?[adehlms]?";

/// docling's `_LIST_ITEM_PATTERN` (docling#4118): besides `*`, `-` and `1.`,
/// AsciiDoc's dotted (`.`, `..`, `...`) and lettered/roman (`a.`, `i.`) ordered
/// markers open list items.
const LIST_ITEM: &str = r"^(\s*)(\*|-|\.+|\d+\.|\w+\.)\s+(.*)";

/// The delimiter opening and closing a literal block.
const LITERAL_FENCE: &str = "....";

#[derive(Default)]
pub struct AsciiDocBackend {
    /// When set, an `image::target[]` target is resolved to the actual image
    /// bytes — docling's `AsciiDocBackendOptions.fetch_images` together with
    /// its `enable_local_fetch`/`enable_remote_fetch` (docling#4156). Off by
    /// default, matching docling: a picture is then emitted with no image at
    /// all (until 2.126 docling fabricated a `file://…` `ImageRef` here).
    pub fetch_images: bool,
}

impl DeclarativeBackend for AsciiDocBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let text = source.text()?;
        if self.fetch_images {
            let resolver = FsImageResolver::new(
                source.base_dir().map(|p| p.to_path_buf()),
                source.base_url.clone(),
            );
            Ok(parse(&text, &source.name, &resolver))
        } else {
            Ok(parse(&text, &source.name, &NoFetch))
        }
    }
}

fn parse(text: &str, name: &str, images: &dyn ImageResolver) -> DoclingDocument {
    let mut doc = DoclingDocument::new(name);
    let mut p = Parser {
        images: Some(images),
        parents: vec![None; LEVELS],
        indents: vec![None; LEVELS],
        ..Parser::default()
    };
    // Iterate raw lines, preserving them as docling does (it never strips the
    // trailing newline-only state machine differently from `str::lines`).
    for block in blocks(text) {
        match block {
            Block::Line(line) => p.feed(line, &mut doc),
            Block::Literal(code) => p.feed_literal(code, &mut doc),
        }
    }
    p.finish(&mut doc);
    doc.tree = Some(std::mem::take(&mut p.tree));
    doc
}

/// docling keeps ten section levels (`parents[0..10]`).
const LEVELS: usize = 10;

/// One unit of input: a raw line, or the body of a `....` literal block.
enum Block<'a> {
    Line(&'a str),
    Literal(String),
}

/// docling's `_iter_blocks`: fold each `....`-delimited run of lines into a
/// single literal block. An unterminated block still yields its body at EOF.
fn blocks(text: &str) -> Vec<Block<'_>> {
    let mut out = Vec::new();
    let mut literal: Option<Vec<&str>> = None;
    for line in text.lines() {
        if line.trim() == LITERAL_FENCE {
            match literal.take() {
                None => literal = Some(Vec::new()),
                Some(body) => out.push(Block::Literal(body.join("\n"))),
            }
            continue;
        }
        match &mut literal {
            Some(body) => body.push(line),
            None => out.push(Block::Line(line)),
        }
    }
    if let Some(body) = literal {
        out.push(Block::Literal(body.join("\n")));
    }
    out
}

/// One open list level — docling's `parents`/`indents` pair for a `ListGroup`.
struct ListLevel {
    /// The indent width of the items that opened this level.
    indent: usize,
    /// How many children the group holds so far. docling-core numbers an
    /// enumerated item by its *position among the group's children*, and a
    /// nested list is a child of the group (not of the preceding item), so it
    /// takes a slot too: `. a` / `.. b` / `. c` renders `1.`, `1.`, `3.`.
    slots: u64,
    /// docling-core renders a group's items as numbers only when the group's
    /// *first* item is enumerated; otherwise every item gets a `-`, whatever
    /// its own marker was.
    enumerated: bool,
}

#[derive(Default)]
struct Parser<'r> {
    text_data: Vec<String>,
    caption_data: Vec<String>,
    table_data: Vec<Vec<String>>,
    in_list: bool,
    in_table: bool,
    /// The currently-open list levels, outermost first.
    levels: Vec<ListLevel>,
    /// Whether the next emitted item starts a fresh list (for the serializer).
    fresh_list: bool,
    /// Index in `doc.nodes` of the most recent list item, so a continuation
    /// block can be attached to it as docling attaches a child.
    last_item: Option<usize>,
    /// Set by a lone `+` inside a list: the next literal block or image stays
    /// inside the open item instead of closing the list.
    list_continuation: bool,
    images: Option<&'r dyn ImageResolver>,
    /// docling's item tree (see the module docs).
    tree: ItemTree,
    /// docling's `parents`: the item open at each level — the title at 0,
    /// headings at their level, list groups at the level after their parent.
    parents: Vec<Option<usize>>,
    /// docling's `indents`: the indent width that opened the list group at a
    /// level (never cleared when a list closes, exactly like upstream).
    indents: Vec<Option<usize>>,
    /// The tree id of the most recent list item — docling's `last_list_item`.
    last_item_tree: Option<usize>,
    /// The unescaped text of the caption [`Self::take_caption`] last returned.
    raw_caption: String,
}

/// What is being fed to [`Parser::close_list_if_needed`] — docling passes a
/// `<literal-block>` sentinel line for the block case, which matches none of
/// its line predicates but is recognised as a continuation block.
enum Trigger<'a> {
    Line(&'a str),
    Literal,
}

impl Parser<'_> {
    /// docling's `_get_current_level`: the level before the first empty slot.
    fn current_level(&self) -> usize {
        (1..LEVELS)
            .find(|&k| self.parents[k].is_none())
            .map_or(0, |k| k - 1)
    }

    /// docling's `_get_current_parent`: the item at [`Self::current_level`].
    fn current_parent(&self) -> Option<usize> {
        (1..LEVELS)
            .find(|&k| self.parents[k].is_none())
            .and_then(|k| self.parents[k - 1])
    }

    /// A text item in the tree (`add_text` / `add_heading` / `add_list_item`).
    fn add_tree_text(
        &mut self,
        parent: Option<usize>,
        label: &str,
        text: &str,
        formatting: Option<Formatting>,
        level: Option<u8>,
        list: Option<ListMeta>,
    ) -> usize {
        self.tree.add(
            parent,
            None,
            TreeKind::Text {
                label: label.into(),
                text: text.into(),
                orig: None,
                formatting,
                hyperlink: None,
                level,
                list,
            },
        )
    }

    /// docling: `parents[level + 1] = add_group(parent=parents[level],
    /// name="list", label=LIST)`, `indents[level + 1] = indent`.
    fn open_list_group(&mut self, level: usize, indent: usize) {
        if level + 1 >= LEVELS {
            return;
        }
        let g = self.tree.add(
            self.parents[level],
            None,
            TreeKind::Group {
                label: "list".into(),
                name: "list".into(),
            },
        );
        self.parents[level + 1] = Some(g);
        self.indents[level + 1] = Some(indent);
    }

    /// A block title claimed by a table / picture / code block: docling adds
    /// it with `add_text(label=CAPTION)` and *no parent* — a body-level item
    /// the floating item then references in `captions`.
    fn add_tree_caption(&mut self, text: &str) -> usize {
        self.add_tree_text(None, "caption", text, None, None, None)
    }

    /// docling's `_close_list_if_needed`: anything that is not a list item, a
    /// blank line, a `+`, or a continuation block claimed by a preceding `+`
    /// ends the open list. Unlike docling ≤ 2.124 the line is *not* swallowed —
    /// it goes on to be parsed as a heading/table/picture/text.
    fn close_list_if_needed(&mut self, trigger: Trigger) {
        if !self.in_list {
            return;
        }
        let keep = match trigger {
            Trigger::Literal => self.list_continuation,
            Trigger::Line(line) => {
                let stripped = line.trim();
                list_item(line).is_some()
                    || stripped.is_empty()
                    || stripped == "+"
                    || (self.list_continuation && is_picture(line))
            }
        };
        if !keep {
            // docling: `parents[level] = None` for the current level only —
            // the indent stays recorded.
            let level = self.current_level();
            self.parents[level] = None;
            self.end_list();
        }
    }

    /// A `....` literal block: docling's `add_code`, nested under the open list
    /// item when a `+` claimed it.
    fn feed_literal(&mut self, code: String, doc: &mut DoclingDocument) {
        self.close_list_if_needed(Trigger::Literal);
        self.flush_text(doc);
        // A pending block title is the code item's *caption* since docling
        // 2.127 (`add_code(caption=…)`), and docling-core's Markdown renders
        // a code item's captions after the block — where 2.126 wrote the
        // title as a text item ahead of it.
        let caption = self.take_caption();
        let tree_caption = if caption.is_some() {
            let raw = self.raw_caption.clone();
            Some(self.add_tree_caption(&raw))
        } else {
            None
        };
        let parent = if self.in_list {
            self.last_item_tree
        } else {
            self.current_parent()
        };
        let code_id = self.tree.add(
            parent,
            None,
            TreeKind::Code {
                text: code.clone(),
                orig: None,
                language: None,
                formatting: None,
                hyperlink: None,
            },
        );
        // docling's `add_code(caption=…)`: the caption is a child reference
        // on the code item.
        if let Some(c) = tree_caption {
            self.tree.items[code_id].children.push(c);
        }
        if !(self.in_list && self.fold_child(doc, &format!("```\n{code}\n```"))) {
            doc.push(Node::Code {
                language: None,
                text: code,
                orig: None,
                pretty: None,
            });
        }
        if let Some(text) = caption {
            doc.push(Node::Caption { text, href: None });
        }
        self.list_continuation = false;
    }

    fn feed(&mut self, line: &str, doc: &mut DoclingDocument) {
        self.close_list_if_needed(Trigger::Line(line));

        // Title: `= ` — the root of the section tree.
        if let Some(rest) = is_title(line) {
            doc.push(Node::Heading {
                level: 1,
                text: escape_text(rest.trim()),
            });
            // docling: `parents[0] = add_text(label=TITLE)` — on the body.
            let id = self.add_tree_text(None, "title", rest.trim(), None, None, None);
            self.parents[0] = Some(id);
            return;
        }

        // Section header: `==+ `. docling ≥ 2.127 parents a heading to its
        // nearest *present* ancestor level, so a `====` straight under a `==`
        // stays where it is read (docling ≤ 2.126 hung it off the body root,
        // which rendered it after the whole section tree).
        if let Some((n, text)) = section_header(line) {
            doc.push(Node::Heading {
                level: n.min(6),
                text: escape_text(text.trim()),
            });
            // docling: level = `=` count − 1, parent = the nearest present
            // ancestor level, deeper levels cleared.
            let level = (usize::from(n) - 1).min(LEVELS - 1);
            let ancestor = (0..level).rev().find_map(|k| self.parents[k]);
            let id = self.add_tree_text(
                ancestor,
                "section_header",
                text.trim(),
                None,
                Some(level as u8),
                None,
            );
            self.parents[level] = Some(id);
            for k in level + 1..LEVELS {
                self.parents[k] = None;
            }
            return;
        }

        // List item
        if let Some(item) = list_item(line) {
            self.push_list_item(item, doc);
            return;
        }
        // A lone `+` inside a list: the next literal block or image belongs to
        // the open item.
        if self.in_list && line.trim() == "+" {
            self.list_continuation = true;
            return;
        }

        // Table start delimiter `|===`
        if line.trim() == "|===" && !self.in_table {
            self.in_table = true;
            return;
        }
        // A table row
        if is_table_line(line) {
            self.in_table = true;
            self.table_data.push(parse_table_line(line));
            return;
        }
        // End of a table (any non-row line, including the closing `|===`)
        if self.in_table {
            self.flush_table(doc);
            // fall through: the line may still be text/caption/etc., except `|===`.
            if line.trim() == "|===" {
                return;
            }
        }

        // Picture
        if let Some(uri) = picture_uri(line) {
            self.push_picture(&uri, doc);
            return;
        }

        // Caption: a line beginning with `.` followed by a non-space (only when
        // none is pending)
        if let Some(rest) = is_caption(line) {
            if self.caption_data.is_empty() {
                self.caption_data.push(rest.to_string());
                return;
            }
        }
        // Continuation of a multi-line caption
        if !line.trim().is_empty() && !self.caption_data.is_empty() {
            self.caption_data.push(line.trim().to_string());
            return;
        }

        // Plain text: blank line flushes the accumulated paragraph
        if line.trim().is_empty() {
            self.flush_text(doc);
        } else {
            self.text_data.push(line.trim().to_string());
        }
    }

    fn push_list_item(&mut self, item: ListItem<'_>, doc: &mut DoclingDocument) {
        // docling's `parents` / `indents` bookkeeping for the tree.
        let mut level = self.current_level();
        if !self.in_list {
            self.in_list = true;
            // A block title pending in front of a list (the `.Procedure` /
            // `.Verification` lead-ins): a list group has no caption slot, so
            // docling ≥ 2.127 emits it as a *bold* paragraph right before the
            // list (2.126 wrote it as a plain caption text).
            if let Some(text) = self.take_caption() {
                doc.push(Node::Paragraph {
                    text: format!("**{text}**"),
                });
                let raw = self.raw_caption.clone();
                let parent = self.parents[level];
                self.add_tree_text(
                    parent,
                    "paragraph",
                    &raw,
                    Some(Formatting {
                        bold: true,
                        ..Formatting::default()
                    }),
                    None,
                    None,
                );
            }
            self.open_list_group(level, item.indent);
        } else if item.indent > self.indents[level].unwrap_or(0) {
            self.open_list_group(level, item.indent);
        } else if item.indent < self.indents[level].unwrap_or(0) {
            while level > 0 && item.indent < self.indents[level].unwrap_or(0) {
                // Only pop while an outer group exists to fall back to;
                // otherwise the current group stays the list root.
                if self.indents[level - 1].is_none() {
                    break;
                }
                self.parents[level] = None;
                self.indents[level] = None;
                level -= 1;
            }
        }
        let tree_parent = self.current_parent();
        let tree_marker =
            numeric_marker(item.marker).map_or(String::new(), |_| item.marker.to_string());
        let id = self.add_tree_text(
            tree_parent,
            "list_item",
            item.text.trim(),
            None,
            None,
            Some(ListMeta {
                enumerated: item.numbered,
                marker: tree_marker,
            }),
        );
        self.last_item_tree = Some(id);
        // The flat path's own list state (`levels`): what the Markdown
        // serializer needs to number and indent the items.
        if self.levels.is_empty() {
            self.levels = vec![ListLevel {
                indent: item.indent,
                slots: 0,
                enumerated: item.numbered,
            }];
            self.fresh_list = true;
        } else if item.indent > self.levels.last().unwrap().indent {
            // The nested group itself occupies a slot in its parent group.
            self.levels.last_mut().unwrap().slots += 1;
            self.levels.push(ListLevel {
                indent: item.indent,
                slots: 0,
                enumerated: item.numbered,
            });
        } else {
            while self.levels.len() > 1 && item.indent < self.levels.last().unwrap().indent {
                self.levels.pop();
            }
        }
        let level = (self.levels.len() - 1) as u8;
        let group = self.levels.last_mut().unwrap();
        group.slots += 1;
        // An explicit numeric marker (`1.`, `12.`) is printed verbatim by
        // docling-core, whatever the item's position; every other enumerated
        // item is numbered by that position.
        let (ordered, number) = match numeric_marker(item.marker) {
            Some(n) => (true, n),
            None => (group.enumerated, group.slots),
        };
        doc.push(Node::ListItem {
            ordered,
            number,
            first_in_list: self.fresh_list,
            text: escape_text(item.text.trim()),
            level,
            marker: numeric_marker(item.marker).map(|_| item.marker.to_string()),
            location: None,
            dclx: None,
            href: None,
            layer: None,
        });
        self.last_item = Some(doc.nodes.len() - 1);
        self.fresh_list = false;
        self.list_continuation = false;
    }

    fn push_picture(&mut self, uri: &str, doc: &mut DoclingDocument) {
        let cap = self.take_caption();
        let image = self.images.and_then(|r| r.resolve(uri));
        // Tree: the caption is a body-level item the picture references; the
        // picture hangs off the open list item or the current section.
        let captions: Vec<usize> = if cap.is_some() {
            let raw = self.raw_caption.clone();
            vec![self.add_tree_caption(&raw)]
        } else {
            Vec::new()
        };
        let parent = if self.in_list {
            self.last_item_tree
        } else {
            self.current_parent()
        };
        self.tree.add(
            parent,
            None,
            TreeKind::Picture {
                captions,
                image: image.clone(),
                classification: None,
                chart: None,
                dpi: None,
            },
        );
        // Inside a list docling nests the picture under the open item, so it
        // is folded into the item's text (see `fold_child`): the marker lands
        // where docling prints it, at the cost of the separate JSON picture
        // item and of any bytes `fetch_images` resolved. A picture in a list is
        // only reachable through a `+` continuation, where docling never has a
        // caption either.
        if self.in_list {
            let mut block = String::new();
            if let Some(cap) = &cap {
                block.push_str(cap);
                block.push('\n');
            }
            block.push_str("<!-- image -->");
            if self.fold_child(doc, &block) {
                self.list_continuation = false;
                return;
            }
        }
        // docling ≥ 2.127 parents the picture to the current section (it used
        // to go to the body root and render after everything).
        doc.push(Node::Picture {
            caption: cap,
            caption_href: None,
            image,
            classification: None,
            caption_parent: Default::default(),
        });
        self.list_continuation = false;
    }

    /// Attach an already-rendered Markdown block to the open list item, the way
    /// the HTML backend folds an `<li>`'s images into the item text: our node
    /// list is flat, so docling's *children of a list item* are carried in the
    /// item's own text, which the Markdown serializer prints unwrapped after
    /// the item line. Each child block is indented to the item's own depth,
    /// matching docling-core's list serializer (which prefixes the nesting
    /// indent to the first line of every part).
    fn fold_child(&mut self, doc: &mut DoclingDocument, block: &str) -> bool {
        let Some(idx) = self.last_item else {
            return false;
        };
        let Some(Node::ListItem { text, level, .. }) = doc.nodes.get_mut(idx) else {
            return false;
        };
        text.push('\n');
        text.push_str(&"    ".repeat(*level as usize));
        text.push_str(block);
        true
    }

    fn end_list(&mut self) {
        self.in_list = false;
        self.levels.clear();
        self.last_item = None;
        self.last_item_tree = None;
        self.list_continuation = false;
    }

    /// Take a pending caption for the picture or table that claims it — the
    /// Markdown-escaped text; the raw joined text stays in `raw_caption` for
    /// the tree item.
    fn take_caption(&mut self) -> Option<String> {
        if self.caption_data.is_empty() {
            return None;
        }
        let cap = self.caption_data.join(" ");
        self.caption_data.clear();
        self.raw_caption = cap.clone();
        Some(escape_text(&cap))
    }

    fn flush_text(&mut self, doc: &mut DoclingDocument) {
        if !self.text_data.is_empty() {
            let text = self.text_data.join(" ");
            self.text_data.clear();
            doc.push(Node::Paragraph {
                text: escape_text(&text),
            });
            let parent = self.current_parent();
            self.add_tree_text(parent, "paragraph", &text, None, None, None);
        }
    }

    fn flush_table(&mut self, doc: &mut DoclingDocument) {
        if !self.table_data.is_empty() {
            // A pending caption is attached to this table and renders before it.
            let mut captions = Vec::new();
            if let Some(cap) = self.take_caption() {
                doc.push(Node::Paragraph { text: cap });
                let raw = self.raw_caption.clone();
                captions.push(self.add_tree_caption(&raw));
            }
            let num_cols = self.table_data.iter().map(Vec::len).max().unwrap_or(0);
            let raw_rows: Vec<Vec<String>> = self.table_data.clone();
            let rows: Vec<Vec<String>> = self
                .table_data
                .drain(..)
                .map(|mut r| {
                    r.resize(num_cols, String::new());
                    r
                })
                .collect();
            let table = Table {
                rows,
                location: None,
                structure: None,
                cell_blocks: None,
                cells: None,
                caption: None,
                caption_parent: Default::default(),
            };
            // docling's `_populate_table_as_grid` writes a cell only where
            // the source row has one — a short row leaves its tail empty in
            // the grid (no cell, no header flag) — so the tree's table carries
            // the ragged cells while the flat rows stay padded for Markdown.
            let cells: Vec<TableCell> = raw_rows
                .iter()
                .enumerate()
                .flat_map(|(r, row)| {
                    row.iter().enumerate().map(move |(c, text)| TableCell {
                        text: text.clone(),
                        bbox: None,
                        start_row: r,
                        start_col: c,
                        row_span: 1,
                        col_span: 1,
                        column_header: r == 0,
                        row_header: false,
                        row_section: false,
                    })
                })
                .collect();
            let parent = self.current_parent();
            self.tree.add(
                parent,
                None,
                TreeKind::Table {
                    table: Table {
                        cells: Some(cells),
                        ..table.clone()
                    },
                    rich_cells: Vec::new(),
                    captions,
                },
            );
            doc.push(Node::Table(table));
        }
        self.in_table = false;
        self.table_data.clear();
    }

    fn finish(&mut self, doc: &mut DoclingDocument) {
        self.flush_text(doc);
        if self.in_table {
            self.flush_table(doc);
        }
    }
}

fn is_title(line: &str) -> Option<&str> {
    line.strip_prefix("= ")
}

/// `== Section` → (number-of-`=`, text). A bare `=` (title) is excluded.
fn section_header(line: &str) -> Option<(u8, String)> {
    if !line.starts_with("==") {
        return None;
    }
    let caps = cached_regex!(r"^(=+)\s+(.*)").captures(line)?;
    let level = caps.get(1)?.as_str().len() as u8;
    Some((level, caps.get(2)?.as_str().to_string()))
}

/// A parsed list item, docling's `_parse_list_item`.
struct ListItem<'a> {
    /// docling's `indent`: the leading whitespace, plus one per extra `.` — a
    /// dotted marker encodes its depth in the marker itself (`..` nests under
    /// `.`), not in the indentation.
    indent: usize,
    marker: &'a str,
    numbered: bool,
    text: &'a str,
}

fn list_item(line: &str) -> Option<ListItem<'_>> {
    let caps = cached_regex!(LIST_ITEM).captures(line)?;
    let marker = caps.get(2)?.as_str();
    let mut indent = caps.get(1)?.as_str().len();
    if marker.starts_with('.') {
        indent += marker.len() - 1;
    }
    Some(ListItem {
        indent,
        marker,
        numbered: !(marker == "*" || marker == "-"),
        text: caps.get(3)?.as_str(),
    })
}

/// The number in an explicit `12.` marker — docling's `marker[:-1].isdigit()`
/// test for the marker it hands to docling-core (which then prints it verbatim
/// instead of numbering the item by position).
fn numeric_marker(marker: &str) -> Option<u64> {
    let digits = marker.strip_suffix('.')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn is_table_line(line: &str) -> bool {
    cached_regex!(&format!(r"^{CELL_SPEC}\|.*\|")).is_match(line)
}

/// Strip cell specifiers glued before a `|`, split on `|`, drop the leading
/// empty field, and trim — exactly as docling's `_parse_table_line`.
fn parse_table_line(line: &str) -> Vec<String> {
    let cleaned = cached_regex!(&format!(r"(^|\s){CELL_SPEC}(\|)")).replace_all(line, "$1$2");
    cleaned
        .split('|')
        .skip(1)
        .map(|c| c.trim().to_string())
        .collect()
}

fn is_picture(line: &str) -> bool {
    line.starts_with("image::")
}

/// The `image::target[attrs]` target. docling falls back to the whole line when
/// the macro is malformed (an unresolvable "uri", which then fetches nothing).
fn picture_uri(line: &str) -> Option<String> {
    if !is_picture(line) {
        return None;
    }
    Some(
        cached_regex!(r"^image::(.+)\[(.*)\]$")
            .captures(line)
            .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
            .unwrap_or_else(|| line.to_string()),
    )
}

fn is_caption(line: &str) -> Option<&str> {
    // `.text`, but not `. text` (an ordered list item) and not a bare `.`.
    let rest = line.strip_prefix('.')?;
    (!rest.starts_with(char::is_whitespace) && !rest.is_empty()).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::images::NoFetch;

    fn md(src: &str) -> String {
        parse(src, "t", &NoFetch).export_to_markdown()
    }

    fn json(src: &str) -> serde_json::Value {
        serde_json::from_str(&parse(src, "t", &NoFetch).export_to_json()).unwrap()
    }

    /// docling's `_parse` tree (verified item for item against docling
    /// 2.129 on this input): the title is the root, a heading hangs off the
    /// nearest present ancestor level, a list opens a `list` group whose
    /// deeper indents nest further groups, items are the innermost group's
    /// children, a block title before a list is a bold paragraph, and a
    /// table's block title is a body-level `caption` the table references.
    #[test]
    fn tree_mirrors_docling_parents_and_indents() {
        let v = json(
            "= Title\n\nAbstract.\n\n== Section 1\n\n* a\n  * b\n* c\n\n\
                      === Deep\n\n.Steps\n. one\n. two\n\n.Table title\n|===\n|H|I|\n|1|2|\n|===\n",
        );
        let texts = v["texts"].as_array().unwrap();
        let by = |i: usize| {
            (
                texts[i]["label"].as_str().unwrap(),
                texts[i]["parent"]["$ref"].as_str().unwrap(),
            )
        };
        assert_eq!(by(0), ("title", "#/body"));
        assert_eq!(by(1), ("paragraph", "#/texts/0"));
        assert_eq!(by(2), ("section_header", "#/texts/0"));
        assert_eq!(texts[2]["level"], 1);
        // `* a` opens groups/0 under the section; `  * b` nests groups/1
        // inside it; `* c` dedents back to groups/0.
        assert_eq!(by(3), ("list_item", "#/groups/0"));
        assert_eq!(by(4), ("list_item", "#/groups/1"));
        assert_eq!(by(5), ("list_item", "#/groups/0"));
        assert_eq!(v["groups"][0]["parent"]["$ref"], "#/texts/2");
        assert_eq!(v["groups"][1]["parent"]["$ref"], "#/groups/0");
        assert_eq!(texts[3]["marker"], "");
        assert_eq!(texts[3]["enumerated"], false);
        // `===` under `==`: level 2, parented to the level-1 heading.
        assert_eq!(by(6), ("section_header", "#/texts/2"));
        assert_eq!(texts[6]["level"], 2);
        // `.Steps` before the dotted list: a bold paragraph under `Deep`; the
        // list group follows it there, its items enumerated with no marker.
        assert_eq!(by(7), ("paragraph", "#/texts/6"));
        assert_eq!(texts[7]["formatting"]["bold"], true);
        assert_eq!(v["groups"][2]["parent"]["$ref"], "#/texts/6");
        assert_eq!(by(8), ("list_item", "#/groups/2"));
        assert_eq!(texts[8]["enumerated"], true);
        assert_eq!(texts[8]["marker"], "");
        // The table's block title: a caption on the *body*, referenced.
        assert_eq!(by(10), ("caption", "#/body"));
        assert_eq!(v["tables"][0]["parent"]["$ref"], "#/texts/6");
        assert_eq!(v["tables"][0]["captions"][0]["$ref"], "#/texts/10");
        assert_eq!(
            v["tables"][0]["data"]["table_cells"][0]["column_header"],
            true
        );
    }

    /// A `+`-continued picture is a child of the open list item (docling's
    /// `parent=last_list_item`), and a short table row leaves its tail
    /// without cells (`_populate_table_as_grid` writes only what the row has).
    #[test]
    fn tree_nests_continued_pictures_and_keeps_ragged_rows() {
        let v = json("= T\n\n. step\n+\nimage::a.png[]\n\n|A|B|C|\n|1|\n");
        assert_eq!(v["pictures"][0]["parent"]["$ref"], "#/texts/1");
        assert_eq!(v["texts"][1]["children"][0]["$ref"], "#/pictures/0");
        // `line.split("|")[1:]` keeps the trailing empty field, so the rows
        // are 4 and 2 cells wide; the short row is not padded to 4.
        let cells = v["tables"][0]["data"]["table_cells"].as_array().unwrap();
        assert_eq!(cells.len(), 6, "4 header cells + 2, no padding");
        assert_eq!(v["tables"][0]["data"]["num_cols"], 4);
        assert_eq!(
            cells
                .iter()
                .filter(|c| c["start_row_offset_idx"] == 1)
                .count(),
            2
        );
    }

    #[test]
    fn literal_block_becomes_a_code_item() {
        assert_eq!(
            md("= T\n\npara\n\n....\nraw literal\n  indented\n....\n\nafter\n"),
            "# T\n\npara\n\n```\nraw literal\n  indented\n```\n\nafter\n"
        );
        // An unterminated block still yields its body (docling's `_iter_blocks`
        // flushes what it has at EOF).
        assert_eq!(md("....\nno close\n"), "```\nno close\n```\n");
        // A block title in front of a literal block is the code item's
        // caption, which docling-core renders after the block (2.127+).
        assert_eq!(
            md(".Literal example\n....\nraw\n....\n"),
            "```\nraw\n```\n\nLiteral example\n"
        );
        assert_eq!(
            md("= T\n\npara\n\n.Cap here\n....\nraw\n....\n\nafter\n"),
            "# T\n\npara\n\n```\nraw\n```\n\nCap here\n\nafter\n"
        );
    }

    #[test]
    fn plus_keeps_a_literal_block_and_an_image_inside_the_item() {
        // docling nests both under the open list item, so they render between
        // the item lines with no blank line and the numbering runs on.
        assert_eq!(
            md(". one\n+\n....\ncode\n....\n. two\n+\nimage::a.png[]\n. three\n"),
            "1. one\n```\ncode\n```\n2. two\n<!-- image -->\n3. three\n"
        );
        // Without the `+` the block closes the list instead.
        assert_eq!(
            md("* one\n\n....\ncode\n....\n"),
            "- one\n\n```\ncode\n```\n"
        );
    }

    #[test]
    fn a_child_block_is_indented_to_its_item_depth() {
        // docling-core prefixes the list indent to the first line of every part
        // it emits, the child blocks included.
        assert_eq!(
            md(". outer\n.. inner\n+\n....\ndeep\n....\n.. inner two\n+\nimage::a.png[]\n"),
            "1. outer\n    1. inner\n    ```\ndeep\n```\n    2. inner two\n    <!-- image -->\n"
        );
    }

    #[test]
    fn dotted_and_lettered_markers_open_ordered_items() {
        // docling#4118 widened the marker set; a dotted marker carries its own
        // depth (`..` nests under `.`), and the nested group takes a position in
        // the parent group, so the item after it is numbered past the gap.
        assert_eq!(
            md(". one\n. two\n.. nested\n. three\na. lettered\n"),
            "1. one\n2. two\n    1. nested\n4. three\n5. lettered\n"
        );
    }

    #[test]
    fn an_explicit_numeric_marker_is_printed_verbatim() {
        // docling hands docling-core the source marker for `\d+.` items only,
        // and it prints those instead of numbering them by position.
        assert_eq!(
            md("1. one\n2. two\n.. sub\n"),
            "1. one\n2. two\n    1. sub\n"
        );
        // A group whose *first* item is a bullet renders every positional item
        // as a bullet, whatever its own marker was.
        assert_eq!(md("* bullet\n. dotted\n"), "- bullet\n- dotted\n");
    }

    #[test]
    fn a_broken_run_of_explicit_markers_stays_one_list() {
        // #385: the list is the group this backend tracked, whatever the
        // explicit markers say — docling prints them verbatim in one list. The
        // Markdown serializer used to read the jump as a new list and insert a
        // blank line (a deviation documented until the boundary guesses went).
        assert_eq!(md("1. one\n5. five\n"), "1. one\n5. five\n");
        // Mixed markers in one group are one list too.
        assert_eq!(
            md("* bullet one\n1. explicit one\n* bullet two\n"),
            "- bullet one\n1. explicit one\n- bullet two\n"
        );
    }

    /// docling ≥ 2.127: a block title in front of a list is a bold paragraph
    /// (a list group has no caption slot); in front of a literal block it is
    /// still the code item's caption text.
    #[test]
    fn a_block_title_in_front_of_a_list_is_a_bold_paragraph() {
        assert_eq!(
            md(".Procedure\n\n. one\n\n.Verification\n\n* check\n"),
            "**Procedure**\n\n1. one\n\n**Verification**\n\n- check\n"
        );
    }

    /// docling ≥ 2.127 parents a level-skipping heading to its nearest present
    /// ancestor and a picture to the current section, so both stay in reading
    /// order (2.126 hung them off the body root, after the whole tree).
    #[test]
    fn skipped_levels_and_pictures_keep_reading_order() {
        assert_eq!(
            md("= T\n\n== A\n\n==== Deep\n\ntext\n\n== B\n\n.Cap\nimage::x.png[]\n\nafter\n"),
            "# T\n\n## A\n\n#### Deep\n\ntext\n\n## B\n\nCap\n\n<!-- image -->\n\nafter\n"
        );
    }

    /// docling#4290 (2.129): a rowspan-only cell specifier (`.2+|`) is a
    /// specifier, not cell text; a `2.3+|` span and a bare `|` still read.
    #[test]
    fn rowspan_only_cell_specifiers_are_stripped() {
        assert_eq!(parse_table_line(".2+|A |B"), vec!["A", "B"]);
        assert_eq!(parse_table_line("2.3+|D ^.^h|E"), vec!["D", "E"]);
        assert!(is_table_line(".2+|A |B"));
        assert_eq!(
            md("|===\n.2+|A |B\n|===\n"),
            "| A   | B   |\n|-----|-----|\n"
        );
    }

    #[test]
    fn a_non_list_line_ends_the_list_without_being_swallowed() {
        // docling ≤ 2.124 dropped this line; docling#4118 parses it (a blank
        // line or a `+` leaves the list open instead).
        assert_eq!(
            md("= T\n\n* one\n* two\n\n== Head\n\npara\n"),
            "# T\n\n- one\n- two\n\n## Head\n\npara\n"
        );
    }

    #[test]
    fn a_lone_word_and_period_stays_a_paragraph() {
        // `\w+\.` + `\s+` matches a bare "Intro." only when the line still
        // carries its newline, which is how docling reads a *file* but not a
        // stream — where it agrees with us and keeps the paragraph.
        assert_eq!(md("= T\n\nIntro.\n"), "# T\n\nIntro.\n");
        // A first word ending in a period *does* open an item, as docling's own
        // widened pattern does on either input — and it is enumerated, so the
        // group numbers it.
        assert_eq!(md("Fig. 1 shows the result\n"), "1. 1 shows the result\n");
    }

    #[test]
    fn an_image_carries_no_imageref_until_images_are_fetched() {
        let png = {
            let img = image::RgbImage::from_pixel(2, 3, image::Rgb([1, 2, 3]));
            let mut buf = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(img)
                .write_to(&mut buf, image::ImageFormat::Png)
                .unwrap();
            buf.into_inner()
        };
        let dir = std::env::temp_dir().join(format!("docling-adoc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.png"), &png).unwrap();
        let src = dir.join("doc.asciidoc");
        std::fs::write(&src, "= T\n\nimage::a.png[Alt]\n").unwrap();

        let picture = |doc: DoclingDocument| {
            doc.nodes
                .into_iter()
                .find_map(|n| match n {
                    Node::Picture { image, .. } => Some(image),
                    _ => None,
                })
                .expect("a picture")
        };
        let source = SourceDocument::from_file(&src).unwrap();
        // docling#4156: off by default the picture carries no image at all (it
        // used to get a fabricated `file://…` ImageRef).
        assert!(picture(AsciiDocBackend::default().convert(&source).unwrap()).is_none());
        let fetched = picture(
            AsciiDocBackend { fetch_images: true }
                .convert(&source)
                .unwrap(),
        )
        .expect("the local image is read");
        assert_eq!((fetched.width, fetched.height), (2, 3));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
