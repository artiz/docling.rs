//! docling's item tree for Markdown — the structure the JSON export serializes.
//!
//! `markdown.rs` renders pulldown-cmark's GFM events into the flat node stream
//! Markdown / DocLang / LaTeX read. Upstream's `MarkdownDocumentBackend` walks
//! marko's **CommonMark** AST (`_iterate_elements`) and hands docling-core a
//! very different shape: a heading or list item is created lazily when its
//! first text run arrives, a paragraph of several inline runs is an `inline`
//! group of one text item per run (each with `formatting` / `hyperlink`), a
//! code span is a `code` item, a nested list hangs off the previous item of
//! the outer list, a pipe table is recognised from the paragraph *text* (marko
//! has no table syntax) and always sits on the body, hard/soft line breaks
//! merge the following run into the previous text item, and every item is
//! numbered in creation order.
//!
//! This module reproduces that walk. It parses the text once more as plain
//! CommonMark (no tables, no strikethrough — what marko sees, so `~~x~~`
//! stays text and a pipe table is a paragraph of `|` lines), rebuilds marko's
//! element tree from the events, and ports `_iterate_elements` call for call
//! into a [`docling_core::tree::ItemTree`].
//!
//! Not reproduced: a document with a raw HTML block. Upstream then exports
//! the parsed document to HTML and re-converts it with the HTML backend, so
//! its JSON is the HTML backend's tree over docling-core's HTML serializer —
//! there is no HTML export here to feed that. Such a document keeps the flat
//! export's JSON ([`build_tree`] returns `None`).

use std::collections::HashMap;

use docling_core::tree::{Formatting, ItemTree, ListMeta, TreeKind};
use docling_core::{Table, TableCell};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::html_tree::{detect_code_language, docling_href};
use super::markdown::unescape_entities;

/// marko's element classes, as docling's walk tells them apart.
enum El {
    Document(Vec<El>),
    Heading {
        level: u8,
        children: Vec<El>,
    },
    Paragraph(Vec<El>),
    List {
        ordered: bool,
        start: u64,
        items: Vec<El>,
    },
    ListItem(Vec<El>),
    /// `Quote`, `ThematicBreak`, `AutoLink`, `LinkRefDef`, inline HTML, …:
    /// "some other element" — closes a pending table, children walked.
    Other(Vec<El>),
    CodeBlock {
        lang: Option<String>,
        text: String,
    },
    HtmlBlock,
    /// `RawText` or `Literal` (an escaped character is a node of its own).
    Text(String),
    Emphasis(Vec<El>),
    Strong(Vec<El>),
    Link {
        dest: String,
        children: Vec<El>,
    },
    Image {
        title: String,
        children: Vec<El>,
    },
    CodeSpan(String),
    LineBreak {
        soft: bool,
    },
}

impl El {
    fn children(&self) -> &[El] {
        match self {
            El::Document(c)
            | El::Paragraph(c)
            | El::ListItem(c)
            | El::Other(c)
            | El::Emphasis(c)
            | El::Strong(c) => c,
            El::Heading { children, .. }
            | El::Link { children, .. }
            | El::Image { children, .. } => children,
            El::List { items, .. } => items,
            _ => &[],
        }
    }

    fn is_inline(&self) -> bool {
        matches!(
            self,
            El::Text(_)
                | El::Emphasis(_)
                | El::Strong(_)
                | El::Link { .. }
                | El::Image { .. }
                | El::CodeSpan(_)
                | El::LineBreak { .. }
        ) || matches!(self, El::Other(c) if c.is_empty())
    }
}

/// Rebuild marko's tree from pulldown's CommonMark events.
fn parse(text: &str) -> El {
    // Contiguous text events merge into one `RawText`; an escape leaves an
    // offset gap (the backslash), so the pieces around it stay separate — a
    // marko `Literal` (see `markdown.rs`).
    let raw: Vec<(Event, std::ops::Range<usize>)> = Parser::new_ext(text, Options::empty())
        .into_offset_iter()
        .collect();
    let mut events: Vec<Event> = Vec::with_capacity(raw.len());
    let mut k = 0;
    while k < raw.len() {
        if matches!(raw[k].0, Event::Text(_)) {
            let mut merged = String::new();
            let mut end = raw[k].1.start;
            while let Some((Event::Text(t), range)) = raw.get(k) {
                if range.start != end {
                    break;
                }
                merged.push_str(t);
                end = range.end;
                k += 1;
            }
            events.push(Event::Text(merged.into()));
        } else {
            events.push(raw[k].0.clone());
            k += 1;
        }
    }
    let mut i = 0;
    El::Document(parse_children(&events, &mut i, 0))
}

/// Deepest container nesting rebuilt — `markdown.rs`'s `MAX_NESTING`
/// (markdown-it's, which docling's parser runs on): a 5 000-level list or
/// 50 000 `>` would otherwise overflow the stack here as it did there.
const MAX_NESTING: u16 = 100;

fn parse_children(events: &[Event], i: &mut usize, depth: u16) -> Vec<El> {
    let mut out = Vec::new();
    while *i < events.len() {
        match &events[*i] {
            Event::End(_) => {
                *i += 1;
                return out;
            }
            Event::Start(_) if depth >= MAX_NESTING => {
                super::markdown::skip_subtree(events, i);
                out.push(El::Other(Vec::new()));
            }
            Event::Start(tag) => {
                let tag = tag.clone();
                *i += 1;
                out.push(parse_container(tag, events, i, depth + 1));
            }
            Event::Text(t) => {
                out.push(El::Text(t.to_string()));
                *i += 1;
            }
            Event::Code(t) => {
                out.push(El::CodeSpan(t.to_string()));
                *i += 1;
            }
            Event::SoftBreak => {
                out.push(El::LineBreak { soft: true });
                *i += 1;
            }
            Event::HardBreak => {
                out.push(El::LineBreak { soft: false });
                *i += 1;
            }
            // Inline HTML, a thematic break, a footnote/task marker: marko's
            // "other" elements, with no children docling would walk.
            _ => {
                out.push(El::Other(Vec::new()));
                *i += 1;
            }
        }
    }
    out
}

fn parse_container(tag: Tag, events: &[Event], i: &mut usize, depth: u16) -> El {
    match tag {
        Tag::Paragraph => El::Paragraph(parse_children(events, i, depth)),
        Tag::Heading { level, .. } => El::Heading {
            level: heading_level(level),
            children: parse_children(events, i, depth),
        },
        Tag::List(start) => El::List {
            ordered: start.is_some(),
            start: start.unwrap_or(1),
            items: parse_children(events, i, depth),
        },
        // marko always wraps an item's text in a `Paragraph`; pulldown puts a
        // tight item's inline content straight under the item.
        Tag::Item => {
            let children = parse_children(events, i, depth);
            let mut wrapped: Vec<El> = Vec::new();
            let mut para: Vec<El> = Vec::new();
            for child in children {
                if child.is_inline() {
                    para.push(child);
                } else {
                    if !para.is_empty() {
                        wrapped.push(El::Paragraph(std::mem::take(&mut para)));
                    }
                    wrapped.push(child);
                }
            }
            if !para.is_empty() {
                wrapped.push(El::Paragraph(para));
            }
            El::ListItem(wrapped)
        }
        Tag::BlockQuote(_) => El::Other(parse_children(events, i, depth)),
        Tag::CodeBlock(kind) => {
            let lang = match kind {
                CodeBlockKind::Fenced(info) => {
                    let lang = info.split_whitespace().next().unwrap_or("");
                    (!lang.is_empty()).then(|| lang.to_string())
                }
                CodeBlockKind::Indented => None,
            };
            let mut code = String::new();
            while *i < events.len() && !matches!(events[*i], Event::End(TagEnd::CodeBlock)) {
                if let Event::Text(t) = &events[*i] {
                    code.push_str(t);
                }
                *i += 1;
            }
            *i += 1;
            El::CodeBlock { lang, text: code }
        }
        Tag::HtmlBlock => {
            while *i < events.len() && !matches!(events[*i], Event::End(TagEnd::HtmlBlock)) {
                *i += 1;
            }
            *i += 1;
            El::HtmlBlock
        }
        Tag::Emphasis => El::Emphasis(parse_children(events, i, depth)),
        Tag::Strong => El::Strong(parse_children(events, i, depth)),
        // marko's `AutoLink` (`<https://…>`) is not a `Link`: no hyperlink,
        // its text is an ordinary run.
        Tag::Link {
            link_type: pulldown_cmark::LinkType::Autolink | pulldown_cmark::LinkType::Email,
            ..
        } => El::Other(parse_children(events, i, depth)),
        Tag::Link { dest_url, .. } => El::Link {
            dest: dest_url.to_string(),
            children: parse_children(events, i, depth),
        },
        Tag::Image { title, .. } => El::Image {
            title: title.to_string(),
            children: parse_children(events, i, depth),
        },
        _ => El::Other(parse_children(events, i, depth)),
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// `_only_plain_line_breaks`: text runs joined only by line breaks, with at
/// least one break — content the pending-line-break merge handles alone, so
/// no `inline` group is opened for it.
fn only_plain_line_breaks(children: &[El]) -> bool {
    children.iter().any(|c| matches!(c, El::LineBreak { .. }))
        && children
            .iter()
            .all(|c| matches!(c, El::Text(_) | El::LineBreak { .. }))
}

/// `_inline_text`: an inline node's text with its markers dropped.
fn inline_text(el: &El, out: &mut String) {
    match el {
        El::Text(t) | El::CodeSpan(t) => out.push_str(t),
        other => other.children().iter().for_each(|c| inline_text(c, out)),
    }
}

/// `_split_table_row`: an empty edge field comes from an edge pipe and is dropped.
fn split_table_row(row: &str) -> Vec<String> {
    let mut cells: Vec<&str> = row.split('|').collect();
    if cells.first().is_some_and(|c| c.trim().is_empty()) {
        cells.remove(0);
    }
    if cells.last().is_some_and(|c| c.trim().is_empty()) {
        cells.pop();
    }
    cells.into_iter().map(|c| c.trim().to_string()).collect()
}

/// `_is_delimiter_row`: every cell is `:?-+:?`.
fn is_delimiter_row(row: &str) -> bool {
    let cells = split_table_row(row);
    !cells.is_empty()
        && cells.iter().all(|c| {
            let body = c.strip_prefix(':').unwrap_or(c);
            let body = body.strip_suffix(':').unwrap_or(body);
            !body.is_empty() && body.bytes().all(|b| b == b'-')
        })
}

/// `_starts_pipeless_table`: a paragraph whose first line holds pipes but no
/// leading one and whose second line is a delimiter row of the same width.
fn starts_pipeless_table(children: &[El]) -> bool {
    let mut lines: Vec<String> = vec![String::new()];
    for child in children {
        if matches!(child, El::LineBreak { .. }) {
            if lines.len() == 2 {
                break;
            }
            lines.push(String::new());
        } else {
            let last = lines.last_mut().expect("one line");
            inline_text(child, last);
        }
    }
    if lines.len() < 2 || lines[0].trim_start().starts_with('|') {
        return false;
    }
    if !lines[0].contains('|') || !is_delimiter_row(&lines[1]) {
        return false;
    }
    split_table_row(&lines[0]).len() == split_table_row(&lines[1]).len()
}

/// A lazily created container, made when its first text run arrives.
enum Pending {
    Heading(u8),
    ListItem { enumerated: bool, marker: String },
}

struct Walker {
    tree: ItemTree,
    in_table: bool,
    in_pipeless_table: bool,
    table_buffer: Vec<String>,
    pending_hard_break: bool,
    pending_soft_break: bool,
    html_blocks: usize,
    creation_stack: Vec<Pending>,
    list_ordered: HashMap<usize, bool>,
    list_start: HashMap<usize, u64>,
    list_counter: HashMap<usize, u64>,
    list_last_item: HashMap<usize, usize>,
}

/// Build docling's item tree for a Markdown document, or `None` when it holds
/// a raw HTML block (see the module docs).
pub(super) fn build_tree(text: &str) -> Option<ItemTree> {
    let root = parse(text);
    let mut w = Walker {
        tree: ItemTree::default(),
        in_table: false,
        in_pipeless_table: false,
        table_buffer: Vec::new(),
        pending_hard_break: false,
        pending_soft_break: false,
        html_blocks: 0,
        creation_stack: Vec::new(),
        list_ordered: HashMap::new(),
        list_start: HashMap::new(),
        list_counter: HashMap::new(),
        list_last_item: HashMap::new(),
    };
    w.iterate(&root, None, None, None);
    w.close_table();
    (w.html_blocks == 0).then_some(w.tree)
}

fn text_kind(
    label: &str,
    text: &str,
    formatting: Option<Formatting>,
    hyperlink: Option<&str>,
    list: Option<ListMeta>,
    level: Option<u8>,
) -> TreeKind {
    TreeKind::Text {
        label: label.into(),
        text: text.into(),
        orig: None,
        formatting,
        hyperlink: hyperlink.map(str::to_string),
        level,
        list,
    }
}

impl Walker {
    /// `_close_table`: the buffered `|` lines become one table on the body —
    /// row 0 the header, row 1 (the delimiter) skipped, the rest the body,
    /// every row cut or padded to the header's width (GFM), cells unescaped.
    fn close_table(&mut self) {
        self.in_pipeless_table = false;
        if !self.in_table {
            return;
        }
        let mut result: Vec<Vec<String>> = Vec::new();
        for (n, row) in self.table_buffer.iter().enumerate() {
            if n != 1 {
                result.push(split_table_row(row));
            }
        }
        if let Some(width) = result.first().map(Vec::len) {
            for row in &mut result {
                row.resize(width, String::new());
            }
        }
        let mut cells = Vec::new();
        for (r, row) in result.iter().enumerate() {
            for (c, value) in row.iter().enumerate() {
                cells.push(TableCell {
                    text: unescape_entities(value.trim()),
                    bbox: None,
                    start_row: r,
                    start_col: c,
                    row_span: 1,
                    col_span: 1,
                    column_header: r == 0,
                    row_header: false,
                    row_section: false,
                });
            }
        }
        self.in_table = false;
        self.table_buffer.clear();
        if !cells.is_empty() {
            let rows: Vec<Vec<String>> = result
                .iter()
                .map(|row| row.iter().map(|v| unescape_entities(v.trim())).collect())
                .collect();
            self.tree.add(
                None,
                None,
                TreeKind::Table {
                    table: Table {
                        rows,
                        cells: Some(cells),
                        ..Table::default()
                    },
                    rich_cells: Vec::new(),
                    captions: Vec::new(),
                },
            );
        }
    }

    fn create_list_item(
        &mut self,
        parent: Option<usize>,
        text: &str,
        enumerated: bool,
        marker: &str,
        formatting: Option<Formatting>,
        hyperlink: Option<&str>,
    ) -> usize {
        self.tree.add(
            parent,
            None,
            text_kind(
                "list_item",
                text,
                formatting,
                hyperlink,
                Some(ListMeta {
                    enumerated,
                    marker: marker.to_string(),
                }),
                None,
            ),
        )
    }

    /// `_create_heading_item`: level 1 is docling's `title`, deeper ones a
    /// `section_header` of level − 1.
    fn create_heading_item(
        &mut self,
        parent: Option<usize>,
        text: &str,
        level: u8,
        formatting: Option<Formatting>,
        hyperlink: Option<&str>,
    ) -> usize {
        let kind = if level == 1 {
            text_kind("title", text, formatting, hyperlink, None, None)
        } else {
            text_kind(
                "section_header",
                text,
                formatting,
                hyperlink,
                None,
                Some(level - 1),
            )
        };
        self.tree.add(parent, None, kind)
    }

    /// `_flush_creation_stack`: create the pending items, innermost last, with
    /// the run's text; a list item becomes the parent for what follows.
    fn flush_creation_stack(
        &mut self,
        snippet: &str,
        mut parent: Option<usize>,
        formatting: Option<Formatting>,
        hyperlink: Option<&str>,
    ) -> Option<usize> {
        while let Some(pending) = self.creation_stack.pop() {
            match pending {
                Pending::ListItem { enumerated, marker } => {
                    let parent_ref = parent;
                    let item = self.create_list_item(
                        parent, snippet, enumerated, &marker, formatting, hyperlink,
                    );
                    parent = Some(item);
                    if let Some(p) = parent_ref {
                        self.list_last_item.insert(p, item);
                        *self.list_counter.entry(p).or_insert(0) += 1;
                    }
                }
                Pending::Heading(level) => {
                    // Not kept as the parent: marko does not nest a section's
                    // content under its heading.
                    self.create_heading_item(parent, snippet, level, formatting, hyperlink);
                }
            }
        }
        parent
    }

    /// `_iterate_elements`.
    fn iterate(
        &mut self,
        el: &El,
        mut parent: Option<usize>,
        mut formatting: Option<Formatting>,
        mut hyperlink: Option<String>,
    ) {
        match el {
            El::Heading { level, children } if !children.is_empty() => {
                self.close_table();
                if children.len() > 1 {
                    // The inline group is created further down, under it.
                    parent = Some(self.create_heading_item(
                        parent,
                        "",
                        *level,
                        formatting,
                        hyperlink.as_deref(),
                    ));
                } else {
                    self.creation_stack.push(Pending::Heading(*level));
                }
            }
            El::List {
                ordered,
                start,
                items,
            } => {
                let has_non_empty = items
                    .iter()
                    .any(|i| matches!(i, El::ListItem(c) if !c.is_empty()));
                self.close_table();
                if has_non_empty {
                    let group = self.tree.add(
                        parent,
                        None,
                        TreeKind::Group {
                            label: "list".into(),
                            name: "list".into(),
                        },
                    );
                    self.list_ordered.insert(group, *ordered);
                    if *ordered {
                        self.list_start.insert(group, *start);
                    }
                    parent = Some(group);
                }
            }
            El::ListItem(children) if matches!(children.first(), Some(El::Paragraph(c)) if !c.is_empty()) =>
            {
                self.close_table();
                let enumerated = parent
                    .and_then(|p| self.list_ordered.get(&p).copied())
                    .unwrap_or(false);
                let mut marker = String::new();
                if let (true, Some(p)) = (enumerated, parent) {
                    let start = self.list_start.get(&p).copied().unwrap_or(1);
                    let count = self.list_counter.get(&p).copied().unwrap_or(0);
                    marker = format!("{}.", start + count);
                }
                let Some(El::Paragraph(para)) = children.first() else {
                    unreachable!()
                };
                // The inline group is created further down; plain runs joined
                // by line breaks merge into one item instead.
                if para.len() > 1 && !only_plain_line_breaks(para) {
                    let parent_ref = parent;
                    let item = self.create_list_item(
                        parent,
                        "",
                        enumerated,
                        &marker,
                        formatting,
                        hyperlink.as_deref(),
                    );
                    parent = Some(item);
                    if let Some(p) = parent_ref {
                        self.list_last_item.insert(p, item);
                        *self.list_counter.entry(p).or_insert(0) += 1;
                    }
                } else {
                    self.creation_stack
                        .push(Pending::ListItem { enumerated, marker });
                }
            }
            El::Image { title, .. } => {
                self.close_table();
                // The `"title"` is the caption (on the body, like every
                // `add_text` without a parent); the alt text runs are walked
                // as the picture's children afterwards. No image payload:
                // upstream fetches none by default.
                let mut captions = Vec::new();
                if !title.is_empty() {
                    let cap = self.tree.add(
                        None,
                        None,
                        text_kind(
                            "caption",
                            &unescape_entities(title),
                            formatting,
                            hyperlink.as_deref(),
                            None,
                            None,
                        ),
                    );
                    captions.push(cap);
                }
                self.tree.add(
                    parent,
                    None,
                    TreeKind::Picture {
                        captions,
                        image: None,
                        classification: None,
                        chart: None,
                        dpi: None,
                    },
                );
            }
            El::Emphasis(_) => {
                let mut f = formatting.unwrap_or_default();
                f.italic = true;
                formatting = Some(f);
            }
            El::Strong(_) => {
                let mut f = formatting.unwrap_or_default();
                f.bold = true;
                formatting = Some(f);
            }
            El::Link { dest, .. } => {
                hyperlink = Some(docling_href(dest));
            }
            El::Text(original) => {
                let mut snippet = unescape_entities(original.trim());
                let is_table_row = !snippet.is_empty()
                    && (self.in_pipeless_table
                        || (snippet.contains('|')
                            && (self.in_table || original.trim_start().starts_with('|'))));
                if is_table_row {
                    self.in_table = true;
                }
                if self.in_table && !snippet.is_empty() {
                    // Formatted cell content arrives as separate runs, each
                    // appended to the row being buffered.
                    snippet = original.trim().to_string();
                    match self.table_buffer.last_mut() {
                        Some(last) => last.push_str(&snippet),
                        None => self.table_buffer.push(snippet),
                    }
                } else if !snippet.is_empty() {
                    self.close_table();
                    if !self.creation_stack.is_empty() {
                        parent = self.flush_creation_stack(
                            &snippet,
                            parent,
                            formatting,
                            hyperlink.as_deref(),
                        );
                    } else {
                        let last = self.tree.last_text().filter(|&id| {
                            let (f, h) = match &self.tree.items[id].kind {
                                TreeKind::Text {
                                    formatting,
                                    hyperlink,
                                    ..
                                }
                                | TreeKind::Code {
                                    formatting,
                                    hyperlink,
                                    ..
                                } => (formatting, hyperlink),
                                _ => return false,
                            };
                            *f == formatting && *h == hyperlink
                        });
                        match (self.pending_hard_break, self.pending_soft_break, last) {
                            (true, _, Some(id)) => self.append_text(id, "\n", &snippet),
                            (false, true, Some(id)) => self.append_text(id, " ", &snippet),
                            _ => {
                                let prefix = if self.pending_hard_break { "\n" } else { "" };
                                self.tree.add(
                                    parent,
                                    None,
                                    text_kind(
                                        "text",
                                        &format!("{prefix}{snippet}"),
                                        formatting,
                                        hyperlink.as_deref(),
                                        None,
                                        None,
                                    ),
                                );
                            }
                        }
                    }
                    self.pending_hard_break = false;
                    self.pending_soft_break = false;
                }
            }
            El::CodeSpan(code) => {
                self.close_table();
                let snippet = code.trim().to_string();
                if !self.creation_stack.is_empty() && !snippet.is_empty() {
                    // The span is the container's own text; no separate code item.
                    self.flush_creation_stack(&snippet, parent, formatting, hyperlink.as_deref());
                    return;
                }
                self.tree.add(
                    parent,
                    None,
                    TreeKind::Code {
                        text: snippet,
                        orig: None,
                        language: None,
                        formatting,
                        hyperlink: hyperlink.clone(),
                    },
                );
            }
            El::CodeBlock { lang, text } if !text.trim().is_empty() => {
                self.close_table();
                let snippet = text.trim().to_string();
                // `detect_code_language(text, hint=lang)`: a known hint wins,
                // else the content's own markers.
                let language = lang
                    .as_deref()
                    .filter(|l| docling_core::code_language_label(l) != "unknown")
                    .map(str::to_string)
                    .or_else(|| detect_code_language(&snippet));
                self.tree.add(
                    parent,
                    None,
                    TreeKind::Code {
                        text: snippet,
                        orig: None,
                        language,
                        formatting,
                        hyperlink: hyperlink.clone(),
                    },
                );
            }
            El::LineBreak { soft } => {
                if self.in_table {
                    self.table_buffer.push(String::new());
                } else if *soft {
                    self.pending_soft_break = true;
                } else {
                    self.pending_hard_break = true;
                }
            }
            El::HtmlBlock => {
                self.html_blocks += 1;
                self.close_table();
            }
            El::Heading { .. } | El::ListItem(_) | El::CodeBlock { .. } => {}
            El::Document(_) | El::Paragraph(_) | El::Other(_) => self.close_table(),
        }

        if let El::Paragraph(children) = el {
            // Read by the text branch above while descending; `close_table`
            // clears it.
            self.in_pipeless_table = starts_pipeless_table(children);
        }

        let children = el.children();
        if matches!(el, El::Paragraph(_) | El::Heading { .. })
            && children.len() > 1
            && !only_plain_line_breaks(children)
        {
            parent = Some(self.tree.add(
                parent,
                None,
                TreeKind::Group {
                    label: "inline".into(),
                    name: "group".into(),
                },
            ));
        }

        if matches!(el, El::CodeBlock { .. } | El::Text(_)) {
            return;
        }
        for child in children {
            if matches!(el, El::ListItem(_)) && matches!(child, El::List { .. }) {
                if let Some(last) = parent.and_then(|p| self.list_last_item.get(&p).copied()) {
                    // A nested list hangs off the outer list's last item.
                    parent = Some(last);
                }
            }
            self.iterate(child, parent, formatting, hyperlink.clone());
        }
    }

    /// `doc.texts[-1].text += sep + snippet` (and `orig` alike).
    fn append_text(&mut self, id: usize, sep: &str, snippet: &str) {
        if let TreeKind::Text { text, .. } | TreeKind::Code { text, .. } =
            &mut self.tree.items[id].kind
        {
            text.push_str(sep);
            text.push_str(snippet);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use docling_core::tree::TreeItem;

    fn tree(md: &str) -> ItemTree {
        build_tree(md).expect("a tree")
    }

    fn label(it: &TreeItem) -> String {
        match &it.kind {
            TreeKind::Text { label, .. } => label.clone(),
            TreeKind::Code { .. } => "code".into(),
            TreeKind::Group { label, name } => format!("{label}:{name}"),
            TreeKind::Table { .. } => "table".into(),
            TreeKind::Picture { .. } => "picture".into(),
            TreeKind::FieldRegion { .. } => "field_region".into(),
        }
    }

    fn text(it: &TreeItem) -> &str {
        match &it.kind {
            TreeKind::Text { text, .. } | TreeKind::Code { text, .. } => text,
            _ => "",
        }
    }

    /// docling's `_iterate_elements`: a paragraph of several runs is an
    /// `inline` group of one text per run with its formatting, a code span a
    /// `code` item; a plain paragraph is one text; a title comes lazily from
    /// its first run.
    #[test]
    fn paragraph_runs_become_an_inline_group() {
        let t = tree("# Title\n\nPlain.\n\nFoo *em* **strong** ***both*** and `code`.\n");
        let labels: Vec<String> = t.items.iter().map(label).collect();
        assert_eq!(
            labels,
            [
                "title",
                "text",
                "inline:group",
                "text",
                "text",
                "text",
                "text",
                "text",
                "code",
                "text"
            ]
        );
        assert_eq!(t.body, vec![0, 1, 2]);
        assert_eq!(t.items[2].children, vec![3, 4, 5, 6, 7, 8, 9]);
        let fmt = |i: usize| match &t.items[i].kind {
            TreeKind::Text { formatting, .. } => formatting.map(|f| (f.bold, f.italic)),
            _ => None,
        };
        assert_eq!((text(&t.items[4]), fmt(4)), ("em", Some((false, true))));
        assert_eq!((text(&t.items[5]), fmt(5)), ("strong", Some((true, false))));
        assert_eq!((text(&t.items[6]), fmt(6)), ("both", Some((true, true))));
        assert_eq!(text(&t.items[9]), ".");
        assert_eq!(fmt(3), None, "an unformatted run carries no `formatting`");
    }

    /// Lists: a `list` group with `1.`-style markers counted from the list's
    /// start; a mixed-run item is an empty item over an inline group whose
    /// link run carries the normalized hyperlink; a nested list hangs off the
    /// outer list's last item.
    #[test]
    fn lists_follow_docling() {
        let t = tree("3. Pull the [**repo**](https://x.y)\n4. two\n   - nested\n5. three\n");
        let labels: Vec<String> = t.items.iter().map(label).collect();
        assert_eq!(
            labels,
            [
                "list:list",
                "list_item",
                "inline:group",
                "text",
                "text",
                "list_item",
                "list:list",
                "list_item",
                "list_item"
            ]
        );
        let marker = |i: usize| match &t.items[i].kind {
            TreeKind::Text { list: Some(l), .. } => (l.enumerated, l.marker.clone()),
            _ => panic!("not a list item"),
        };
        assert_eq!(marker(1), (true, "3.".into()));
        assert_eq!(text(&t.items[1]), "");
        assert_eq!(t.items[2].parent, Some(1));
        assert!(
            matches!(&t.items[4].kind, TreeKind::Text { hyperlink: Some(h), formatting: Some(f), .. } if h == "https://x.y/" && f.bold)
        );
        assert_eq!(marker(5), (true, "4.".into()));
        assert_eq!(
            t.items[6].parent,
            Some(5),
            "the nested list hangs off `two`"
        );
        assert_eq!(marker(7), (false, String::new()));
        assert_eq!(marker(8), (true, "5.".into()));
    }

    /// A heading of several runs is created empty with an inline group of its
    /// runs under it; a heading whose only run is a code span takes that text.
    #[test]
    fn headings_with_runs_wrap_an_inline_group() {
        let t = tree("## See [docs](https://e.com/) now\n\n### `code head`\n");
        assert_eq!(label(&t.items[0]), "section_header");
        assert_eq!(text(&t.items[0]), "");
        assert_eq!(label(&t.items[1]), "inline:group");
        assert_eq!(t.items[1].parent, Some(0));
        assert_eq!(text(&t.items[3]), "docs");
        assert_eq!(text(&t.items[5]), "code head");
        assert!(matches!(
            &t.items[5].kind,
            TreeKind::Text { level: Some(2), .. }
        ));
    }

    /// A pipe table is read from the paragraph text (marko has no table
    /// syntax) and always sits on the body; formatted header cells lose their
    /// markers; a hard break merges the next run into the previous text.
    #[test]
    fn tables_sit_on_the_body_and_breaks_merge() {
        let t = tree("- item\n\n| **A** | B |\n|---|---|\n| 1 | 2 |\n\nline one  \nline two\n");
        let labels: Vec<String> = t.items.iter().map(label).collect();
        // The table paragraph has several runs, so upstream opens an inline
        // group for it too — left empty once the runs turn out to be rows.
        assert_eq!(
            labels,
            ["list:list", "list_item", "inline:group", "table", "text"]
        );
        assert!(t.items[2].children.is_empty());
        assert_eq!(t.items[3].parent, None);
        let TreeKind::Table { table, .. } = &t.items[3].kind else {
            panic!()
        };
        let cells = table.cells.as_ref().unwrap();
        assert_eq!(
            cells
                .iter()
                .map(|c| (c.text.as_str(), c.column_header))
                .collect::<Vec<_>>(),
            [("A", true), ("B", true), ("1", false), ("2", false)]
        );
        assert_eq!(text(&t.items[4]), "line one\nline two");
    }

    /// A raw HTML block means upstream re-converts through the HTML backend;
    /// no tree is built (the flat export stays).
    #[test]
    fn html_blocks_yield_no_tree() {
        assert!(build_tree("para\n\n<div>hi</div>\n").is_none());
        assert!(build_tree("para\n").is_some());
    }

    /// Deep nesting is cut off instead of overflowing the stack.
    #[test]
    fn deep_nesting_is_bounded() {
        let deep = ">".repeat(50_000) + " x\n";
        assert!(build_tree(&deep).is_some());
    }
}
