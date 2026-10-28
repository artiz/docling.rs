//! RIS legal XML (#902) — the `risdok` documents of Austria's federal Legal
//! Information System (Rechtsinformationssystem des Bundes, the open-data
//! service `ogd.ris.bka.gv.at`): court and authority decisions and federal
//! law, namespace `http://www.bka.gv.at`. docling has no RIS reader, so this
//! is a docling.rs extension; a bare `.xml` whose root is `<risdok>` comes
//! here ([`looks_like_ris`], read by the `xml` route's dialect sniff), where
//! it used to be kept as one `xml` code block.
//!
//! The document is read as its markup declares it, in source order:
//!
//! * `nutzdaten` (the content) holds `abschnitt` sections — each one a
//!   `section` group named `abschnitt {nr}` in the item tree, so every item
//!   keeps the section it came from; `metadaten` and `layoutdaten` (page
//!   geometry) carry no content and are skipped;
//! * `kzinhalt` / `fzinhalt` (Kopf-/Fußzeile, the running header and footer
//!   of a page variant) are `page_header` / `page_footer` items on the
//!   furniture layer — left out of the Markdown like any page furniture;
//! * `ueberschrift` is a section header (`typ` `titel`/`g1` a level-1 one,
//!   `g2`…`g4` deeper), `absatz` a paragraph whose `b` / `i` / `u` / `sup` /
//!   `sub` runs keep their formatting (an inline group in the item tree) —
//!   a field (`feld`) or `span` reads as its text, a `tab` as a space;
//! * an `absatz` that is a bullet — a glyph (`-`, `–`, `•`, …) or nothing
//!   before its first `tab`, in a hanging indent for the latter — is a list
//!   item, consecutive ones one list; numbered paragraphs (`1.1.`, `3.2.1.`)
//!   stay paragraphs, their numbers part of the legal text;
//! * a `table` (HTML-shaped `tr` / `td` with `absatz` cell content,
//!   `colspan` / `rowspan` honoured) is a table; the zero-height layout row
//!   Word leaves behind is dropped;
//! * `abstand` (vertical spacing) is skipped; an unknown element is read
//!   through — its blocks in place, or one paragraph when it holds only text.
//!
//! The Markdown escapes text as every backend here does (docling's
//! `escape_text`: `_` and the HTML specials) and, unlike docling's, `*` as
//! well: RIS pseudonymizes with stars (`A*** GmbH`, `**** L***berg`,
//! `FN *0*271*h`), which would otherwise open and close emphasis — the
//! plain-text export reads `\*` back as a star. A styled run is wrapped in
//! `**` / `*` against its neighbours with no extra space (docling-core joins
//! an inline group's runs with spaces, which would split `„*x*“`). Table
//! cells keep docling's escaping, as every backend's do.

use docling_core::tree::{Formatting, ItemTree, ListMeta, TreeKind};
use docling_core::{
    inline_paragraph_node, ContentLayer, DoclingDocument, InlineRun, Node, Script, Table, TableCell,
};
use roxmltree::{Document, Node as XmlNode};

use crate::backend::markdown::escape_text;
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;

/// The RIS namespace (`<risdok xmlns="http://www.bka.gv.at">`).
const RIS_NS: &str = "http://www.bka.gv.at";

/// Bullet glyphs an `absatz` may open with before its first `tab`.
const BULLETS: &[&str] = &["-", "–", "—", "•", "·", "*", "o", "▪", "■", "○", "●", "►"];

/// Whether `marker` is a bullet: one of [`BULLETS`], or a single Symbol /
/// Wingdings glyph — Word stores those fonts' bullets in the private-use
/// block U+F000–U+F0FF (`U+F0B7` is Symbol's `•`), and RIS keeps them so.
fn is_bullet(marker: &str) -> bool {
    let mut chars = marker.chars();
    BULLETS.contains(&marker)
        || matches!(
            (chars.next(), chars.next()),
            (Some('\u{f000}'..='\u{f0ff}'), None)
        )
}

/// Whether the head of an XML file is a RIS document's: its first element is
/// `<risdok>` (the RIS namespace on it, or no namespace at all — the
/// open-data files always declare it, hand-trimmed excerpts may not).
pub(crate) fn looks_like_ris(head: &str) -> bool {
    let Some(tag) = crate::backend::feed::first_element_tag(head) else {
        return false;
    };
    let name = tag
        .trim_start_matches('<')
        .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .next()
        .unwrap_or("");
    name.rsplit(':').next() == Some("risdok") && (tag.contains(RIS_NS) || !tag.contains("xmlns"))
}

/// The RIS reader (#902): see the module docs.
pub struct RisBackend;

impl DeclarativeBackend for RisBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let text = source.text()?;
        super::xml_depth::check(&text, "ris")?;
        let dom = Document::parse_with_options(
            &text,
            roxmltree::ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        )
        .map_err(|e| ConversionError::Parse(format!("ris: not well-formed XML ({e})")))?;
        let root = dom.root_element();
        if root.tag_name().name() != "risdok" {
            // Not RIS after all: the generic XML reading, with its warning.
            return crate::backend::text::convert_xml_text(source);
        }
        let mut b = Builder::default();
        for part in root.children().filter(XmlNode::is_element) {
            match part.tag_name().name() {
                "nutzdaten" => {
                    for el in part.children().filter(XmlNode::is_element) {
                        if el.tag_name().name() == "abschnitt" {
                            let name = match el.attribute("nr") {
                                Some(nr) => format!("abschnitt {nr}"),
                                None => "abschnitt".to_string(),
                            };
                            let group = b.add(None, None, group("section", &name));
                            b.blocks(el, Some(group));
                        } else {
                            b.blocks_of(el, None);
                        }
                    }
                }
                // No content: the (empty, in the open-data files) metadata
                // and the page geometry.
                "metadaten" | "layoutdaten" => {}
                _ => b.blocks_of(part, None),
            }
        }
        let mut doc = DoclingDocument::new(&source.name);
        let mut tree = b.tree;
        // Our own tree, so DocLang serializes it too (docling-core's
        // serializer over the tree, #884): formatting runs and the section
        // groups reach it as they reach the JSON.
        tree.doclang = true;
        doc.nodes = b.nodes;
        doc.tree = Some(tree);
        Ok(doc)
    }
}

/// The inline formatting an element puts on its text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Fmt {
    bold: bool,
    italic: bool,
    underline: bool,
    script: Script,
}

impl Fmt {
    fn with(self, tag: &str) -> Self {
        match tag {
            "b" | "strong" | "fett" => Fmt { bold: true, ..self },
            "i" | "em" | "kursiv" => Fmt {
                italic: true,
                ..self
            },
            "u" => Fmt {
                underline: true,
                ..self
            },
            "sup" => Fmt {
                script: Script::Super,
                ..self
            },
            "sub" => Fmt {
                script: Script::Sub,
                ..self
            },
            _ => self,
        }
    }

    fn is_plain(self) -> bool {
        self == Fmt::default()
    }

    fn formatting(self) -> Option<Formatting> {
        (!self.is_plain()).then_some(Formatting {
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            strikethrough: false,
            script: self.script,
        })
    }

    fn run(self, text: &str) -> InlineRun {
        InlineRun {
            text: text.to_string(),
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            script: self.script,
            ..InlineRun::default()
        }
    }
}

/// Collect `el`'s inline content as `(text, formatting)` runs: formatting
/// elements set their style, `tab` / `br` read as a space, `aw` (an empty
/// paragraph mark) as nothing, any other element as its content.
fn collect(el: XmlNode, fmt: Fmt, out: &mut Vec<(String, Fmt)>) {
    for n in el.children() {
        if let Some(t) = n.text().filter(|_| n.is_text()) {
            out.push((t.to_string(), fmt));
        } else if n.is_element() {
            match n.tag_name().name() {
                "tab" | "br" => out.push((" ".to_string(), fmt)),
                "aw" => {}
                tag => collect(n, fmt.with(tag), out),
            }
        }
    }
}

/// The runs, whitespace collapsed to single spaces across run boundaries,
/// the whole trimmed, adjacent runs of one formatting merged and empty ones
/// dropped. A run of nothing but a space (the `tab` between a bold number
/// and a bold heading) joins the run before it rather than splitting the
/// paragraph into a blank item.
fn normalize(raw: Vec<(String, Fmt)>) -> Vec<(String, Fmt)> {
    let mut runs: Vec<(String, Fmt)> = Vec::new();
    let mut prev_space = true;
    for (text, fmt) in raw {
        let mut s = String::new();
        for c in text.chars() {
            if c.is_whitespace() {
                if !prev_space {
                    s.push(' ');
                }
                prev_space = true;
            } else {
                s.push(c);
                prev_space = false;
            }
        }
        if s.is_empty() {
            continue;
        }
        match runs.last_mut() {
            Some((t, f)) if *f == fmt || s == " " => t.push_str(&s),
            _ => runs.push((s, fmt)),
        }
    }
    if let Some((t, _)) = runs.last_mut() {
        let trimmed = t.trim_end().len();
        t.truncate(trimmed);
    }
    runs.retain(|(t, _)| !t.is_empty());
    runs
}

/// The Markdown of a run sequence: each run escaped, the styled ones wrapped
/// in their markers against their neighbours (the edge spaces moved outside
/// the markers). Underline and sub/superscript have no Markdown and print
/// plain, so runs that only differ in those merge first.
fn markdown(runs: &[(String, Fmt)]) -> String {
    let mut merged: Vec<(String, bool, bool)> = Vec::new();
    for (t, f) in runs {
        match merged.last_mut() {
            Some((mt, b, i)) if *b == f.bold && *i == f.italic => mt.push_str(t),
            _ => merged.push((t.clone(), f.bold, f.italic)),
        }
    }
    let mut out = String::new();
    for (t, bold, italic) in merged {
        let marker = match (bold, italic) {
            (true, true) => "***",
            (true, false) => "**",
            (false, true) => "*",
            (false, false) => "",
        };
        let core = t.trim();
        if marker.is_empty() || core.is_empty() {
            out.push_str(&md_escape(&t));
            continue;
        }
        let lead = &t[..t.len() - t.trim_start().len()];
        let trail = &t[t.trim_end().len()..];
        out.push_str(lead);
        out.push_str(marker);
        out.push_str(&md_escape(core));
        out.push_str(marker);
        out.push_str(trail);
    }
    out
}

/// Markdown text: docling's escaping, and the stars (module docs).
fn md_escape(text: &str) -> String {
    escape_text(text).replace('*', "\\*")
}

fn plain(runs: &[(String, Fmt)]) -> String {
    runs.iter().map(|(t, _)| t.as_str()).collect()
}

fn text_kind(label: &str, text: &str, fmt: Option<Formatting>) -> TreeKind {
    TreeKind::Text {
        label: label.into(),
        text: text.into(),
        orig: None,
        formatting: fmt,
        hyperlink: None,
        level: None,
        list: (label == "list_item").then(ListMeta::default),
    }
}

fn group(label: &str, name: &str) -> TreeKind {
    TreeKind::Group {
        label: label.into(),
        name: name.into(),
    }
}

/// The section-header level of an `ueberschrift` `typ`: the judicature's
/// `titel` headings and law's top `g1` outline level are level 1, `g2`…`g4`
/// one deeper each; anything else (`para`, `art`, …) level 1.
fn heading_level(typ: Option<&str>) -> u8 {
    match typ {
        Some("g2") => 2,
        Some("g3") => 3,
        Some("g4") | Some("g5") => 4,
        _ => 1,
    }
}

/// The item tree and the flat nodes, built side by side in source order.
#[derive(Default)]
struct Builder {
    tree: ItemTree,
    nodes: Vec<Node>,
    /// The open list: its tree group and the parent it hangs in.
    list: Option<(usize, Option<usize>)>,
}

impl Builder {
    fn add(&mut self, parent: Option<usize>, layer: Option<ContentLayer>, kind: TreeKind) -> usize {
        self.tree.add(parent, layer, kind)
    }

    /// The blocks of a section (or another container), in order.
    fn blocks(&mut self, el: XmlNode, parent: Option<usize>) {
        for child in el.children().filter(XmlNode::is_element) {
            self.block(child, parent);
        }
        self.list = None;
    }

    /// An element read through: its blocks, or one paragraph when it holds
    /// only inline content.
    fn blocks_of(&mut self, el: XmlNode, parent: Option<usize>) {
        let has_blocks = el.children().any(|c| {
            c.is_element()
                && matches!(
                    c.tag_name().name(),
                    "absatz" | "ueberschrift" | "table" | "kzinhalt" | "fzinhalt" | "abschnitt"
                )
        });
        if has_blocks {
            self.blocks(el, parent);
        } else {
            self.list = None;
            self.paragraph(el, parent);
        }
    }

    fn block(&mut self, el: XmlNode, parent: Option<usize>) {
        let tag = el.tag_name().name();
        if tag != "absatz" {
            self.list = None;
        }
        match tag {
            "kzinhalt" => self.furniture(el, parent, "page_header"),
            "fzinhalt" => self.furniture(el, parent, "page_footer"),
            "ueberschrift" => self.heading(el, parent),
            "absatz" => {
                if !self.list_item(el, parent) {
                    self.list = None;
                    self.paragraph(el, parent);
                }
            }
            "table" => self.table(el, parent),
            "abstand" => {}
            _ => self.blocks_of(el, parent),
        }
    }

    /// A running header or footer: each of its paragraphs one furniture item.
    fn furniture(&mut self, el: XmlNode, parent: Option<usize>, label: &str) {
        for p in el.descendants().filter(|n| n.has_tag_name("absatz")) {
            let mut raw = Vec::new();
            collect(p, Fmt::default(), &mut raw);
            let text = plain(&normalize(raw));
            if text.is_empty() {
                continue;
            }
            self.add(
                parent,
                Some(ContentLayer::Furniture),
                text_kind(label, &text, None),
            );
            self.nodes.push(Node::FurnitureText {
                label: label.into(),
                text: md_escape(&text),
            });
        }
    }

    fn heading(&mut self, el: XmlNode, parent: Option<usize>) {
        let mut raw = Vec::new();
        collect(el, Fmt::default(), &mut raw);
        let text = plain(&normalize(raw));
        if text.is_empty() {
            return;
        }
        let level = heading_level(el.attribute("typ"));
        let mut kind = text_kind("section_header", &text, None);
        if let TreeKind::Text { level: l, .. } = &mut kind {
            *l = Some(level);
        }
        self.add(parent, None, kind);
        self.nodes.push(Node::Heading {
            level: level + 1,
            text: md_escape(&text),
        });
    }

    /// A paragraph: a text item, or an inline group of its runs.
    fn paragraph(&mut self, el: XmlNode, parent: Option<usize>) {
        let mut raw = Vec::new();
        collect(el, Fmt::default(), &mut raw);
        let runs = normalize(raw);
        if runs.is_empty() {
            return;
        }
        match runs.as_slice() {
            [(text, fmt)] => {
                self.add(parent, None, text_kind("text", text, fmt.formatting()));
            }
            _ => {
                let g = self.add(parent, None, group("inline", "group"));
                for (text, fmt) in &runs {
                    self.add(Some(g), None, text_kind("text", text, fmt.formatting()));
                }
            }
        }
        let inline: Vec<InlineRun> = runs.iter().map(|(t, f)| f.run(t)).collect();
        self.nodes
            .push(inline_paragraph_node(markdown(&runs), inline, false));
    }

    /// An `absatz` that is a bullet (module docs) as a list item; `false`
    /// when it is not one.
    fn list_item(&mut self, el: XmlNode, parent: Option<usize>) -> bool {
        let Some(tab) = el.children().find(|n| n.has_tag_name("tab")) else {
            return false;
        };
        let mut before = String::new();
        for n in el.children().take_while(|n| *n != tab) {
            before.push_str(
                &n.descendants()
                    .filter_map(|d| d.text().filter(|_| d.is_text()))
                    .collect::<String>(),
            );
        }
        let marker = before.trim();
        let hanging = el
            .attribute("style")
            .is_some_and(|s| s.replace(' ', "").contains("text-indent:-"));
        if !(is_bullet(marker) || (marker.is_empty() && hanging)) {
            return false;
        }
        let mut raw = Vec::new();
        for n in el.children().skip_while(|n| *n != tab).skip(1) {
            if let Some(t) = n.text().filter(|_| n.is_text()) {
                raw.push((t.to_string(), Fmt::default()));
            } else if n.is_element() {
                match n.tag_name().name() {
                    "tab" | "br" => raw.push((" ".to_string(), Fmt::default())),
                    "aw" => {}
                    tag => collect(n, Fmt::default().with(tag), &mut raw),
                }
            }
        }
        let runs = normalize(raw);
        if runs.is_empty() {
            return false;
        }
        let first_in_list = !matches!(self.list, Some((_, p)) if p == parent);
        if first_in_list {
            let g = self.add(parent, None, group("list", "list"));
            self.list = Some((g, parent));
        }
        let (list, _) = self.list.expect("open list");
        let text = plain(&runs);
        let item = self.add(Some(list), None, text_kind("list_item", &text, None));
        if runs.len() > 1 || !runs[0].1.is_plain() {
            // A styled item: its runs as an inline group under the item.
            if let TreeKind::Text { text, .. } = &mut self.tree.items[item].kind {
                text.clear();
            }
            let g = self.add(Some(item), None, group("inline", "group"));
            for (t, f) in &runs {
                self.add(Some(g), None, text_kind("text", t, f.formatting()));
            }
        }
        self.nodes.push(Node::ListItem {
            ordered: false,
            number: 0,
            first_in_list,
            text: markdown(&runs),
            level: 0,
            marker: None,
            location: None,
            dclx: None,
            href: None,
            layer: None,
        });
        true
    }

    /// A `table`: its rows (`tr`, in `thead` / `tbody` too) as a cell grid,
    /// spans honoured, a cell's paragraphs joined by spaces.
    fn table(&mut self, el: XmlNode, parent: Option<usize>) {
        let rows: Vec<XmlNode> = el
            .descendants()
            .filter(|n| n.has_tag_name("tr"))
            // Only this table's rows, not a nested table's.
            .filter(|tr| {
                tr.ancestors()
                    .skip(1)
                    .find(|a| a.has_tag_name("table"))
                    .is_some_and(|t| t == el)
            })
            .collect();
        let mut cells: Vec<TableCell> = Vec::new();
        // Grid occupancy by earlier rowspans: (row, col) taken.
        let mut taken: std::collections::HashSet<(usize, usize)> = Default::default();
        let mut r = 0usize;
        for tr in rows {
            let tds: Vec<XmlNode> = tr
                .children()
                .filter(|n| n.has_tag_name("td") || n.has_tag_name("th"))
                .collect();
            let texts: Vec<String> = tds.iter().map(|td| cell_text(*td)).collect();
            // Word's zero-height layout row: empty cells that only carry widths.
            if texts.iter().all(String::is_empty)
                && (tr.attribute("height") == Some("0") || tds.iter().all(|td| !td.has_children()))
            {
                continue;
            }
            let mut c = 0usize;
            for (td, text) in tds.iter().zip(texts) {
                while taken.contains(&(r, c)) {
                    c += 1;
                }
                let span = |a: &str| {
                    td.attribute(a)
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .filter(|&n| n > 0)
                        .unwrap_or(1)
                        .min(1000)
                };
                let (rs, cs) = (span("rowspan"), span("colspan"));
                for dr in 0..rs {
                    for dc in 0..cs {
                        taken.insert((r + dr, c + dc));
                    }
                }
                cells.push(TableCell {
                    text,
                    bbox: None,
                    start_row: r,
                    start_col: c,
                    row_span: rs,
                    col_span: cs,
                    column_header: td.has_tag_name("th"),
                    row_header: false,
                    row_section: false,
                });
                c += cs;
            }
            r += 1;
        }
        if cells.is_empty() {
            return;
        }
        let num_rows = cells
            .iter()
            .map(|c| c.start_row + c.row_span)
            .max()
            .unwrap_or(0);
        let num_cols = cells
            .iter()
            .map(|c| c.start_col + c.col_span)
            .max()
            .unwrap_or(0);
        let grid = |escape: bool| -> Vec<Vec<String>> {
            let mut g = vec![vec![String::new(); num_cols]; num_rows];
            for cell in &cells {
                let t = if escape {
                    escape_text(&cell.text)
                } else {
                    cell.text.clone()
                };
                for row in g.iter_mut().skip(cell.start_row).take(cell.row_span) {
                    for slot in row.iter_mut().skip(cell.start_col).take(cell.col_span) {
                        slot.clone_from(&t);
                    }
                }
            }
            g
        };
        let tree_table = Table {
            rows: grid(false),
            cells: Some(cells.clone()),
            ..Table::default()
        };
        self.add(
            parent,
            None,
            TreeKind::Table {
                table: tree_table,
                rich_cells: Vec::new(),
                captions: Vec::new(),
            },
        );
        let flat_rows = grid(true);
        let flat_cells = cells
            .into_iter()
            .map(|c| TableCell {
                text: escape_text(&c.text),
                ..c
            })
            .collect();
        self.nodes.push(Node::Table(Table {
            rows: flat_rows,
            cells: Some(flat_cells),
            ..Table::default()
        }));
    }
}

/// A table cell's text: its paragraphs (or its inline content when it has
/// none), each normalized, joined by a space.
fn cell_text(td: XmlNode) -> String {
    let paras: Vec<XmlNode> = td
        .descendants()
        .filter(|n| n.has_tag_name("absatz"))
        .collect();
    let parts: Vec<String> = if paras.is_empty() {
        let mut raw = Vec::new();
        collect(td, Fmt::default(), &mut raw);
        vec![plain(&normalize(raw))]
    } else {
        paras
            .into_iter()
            .map(|p| {
                let mut raw = Vec::new();
                collect(p, Fmt::default(), &mut raw);
                plain(&normalize(raw))
            })
            .collect()
    };
    parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::InputFormat;

    fn convert(xml: &str) -> DoclingDocument {
        RisBackend
            .convert(&SourceDocument::from_bytes(
                "ris.xml",
                InputFormat::XmlRis,
                xml.as_bytes().to_vec(),
            ))
            .expect("convert")
    }

    fn wrap(body: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><risdok xmlns=\"http://www.bka.gv.at\">\
             <metadaten /><nutzdaten><abschnitt nr=\"1\" typ=\"ns\">{body}</abschnitt>\
             </nutzdaten></risdok>"
        )
    }

    /// The issue's minimal sample: a heading and a paragraph, in order, as
    /// separate items under the section; no markup in the text.
    #[test]
    fn heading_and_paragraph_are_items() {
        let doc = convert(&wrap(
            "<ueberschrift typ=\"titel\" halign=\"j\">Entscheidende Behörde</ueberschrift>\
             <absatz typ=\"erltext\" ct=\"organ\" halign=\"j\">Datenschutzbehörde</absatz>",
        ));
        assert_eq!(
            doc.export_to_markdown(),
            "## Entscheidende Behörde\n\nDatenschutzbehörde\n"
        );
        let json: serde_json::Value = serde_json::from_str(&doc.export_to_json()).unwrap();
        assert_eq!(json["groups"][0]["name"], "abschnitt 1");
        assert_eq!(json["groups"][0]["label"], "section");
        let texts = json["texts"].as_array().unwrap();
        assert_eq!(texts[0]["label"], "section_header");
        assert_eq!(texts[0]["text"], "Entscheidende Behörde");
        assert_eq!(texts[0]["parent"]["$ref"], "#/groups/0");
        assert_eq!(texts[1]["label"], "text");
        assert_eq!(texts[1]["text"], "Datenschutzbehörde");
        assert!(!doc.export_to_text().contains('<'));
    }

    /// Formatting runs reach the JSON as an inline group and the Markdown
    /// as markers hugging the text; a tab reads as a space.
    #[test]
    fn formatting_runs_and_tabs() {
        let doc = convert(&wrap(
            "<absatz><b>1.</b><tab position=\"63.75\" /><u><b>Sachverhalt:</b></u></absatz>\
             <absatz>unter „<i>https://h***.a***.at/</i>“ keine</absatz>",
        ));
        assert_eq!(
            doc.export_to_markdown(),
            "**1. Sachverhalt:**\n\nunter „*https://h\\*\\*\\*.a\\*\\*\\*.at/*“ keine\n"
        );
        let json: serde_json::Value = serde_json::from_str(&doc.export_to_json()).unwrap();
        let texts = json["texts"].as_array().unwrap();
        assert_eq!(texts[0]["text"], "1. ");
        assert_eq!(texts[0]["formatting"]["bold"], true);
        assert_eq!(texts[1]["text"], "Sachverhalt:");
        assert_eq!(texts[1]["formatting"]["underline"], true);
    }

    /// Running headers and footers are page furniture: in the JSON (and
    /// DocLang), not in the Markdown.
    #[test]
    fn headers_and_footers_are_furniture() {
        let doc = convert(&wrap(
            "<kzinhalt typ=\"p\"><absatz><tab/><feld><span>Datenschutzbehörde</span></feld>\
             <tab/><feld><span>04.09.2025</span></feld></absatz></kzinhalt>\
             <fzinhalt typ=\"p\"><absatz>www.ris.bka.gv.at<tab/>Seite <feld><span>2</span></feld>\
             </absatz></fzinhalt><absatz>Body.</absatz>",
        ));
        assert_eq!(doc.export_to_markdown(), "Body.\n");
        let json: serde_json::Value = serde_json::from_str(&doc.export_to_json()).unwrap();
        let texts = json["texts"].as_array().unwrap();
        assert_eq!(texts[0]["label"], "page_header");
        assert_eq!(texts[0]["text"], "Datenschutzbehörde 04.09.2025");
        assert_eq!(texts[0]["content_layer"], "furniture");
        assert_eq!(texts[1]["label"], "page_footer");
        assert_eq!(texts[1]["text"], "www.ris.bka.gv.at Seite 2");
        let dl = doc.export_to_doclang();
        assert!(dl.contains("<page_header>"), "{dl}");
    }

    /// Bullet paragraphs (a glyph before the tab, or none in a hanging
    /// indent) are one list; numbered ones stay paragraphs.
    #[test]
    fn bullets_are_lists_numbers_are_paragraphs() {
        let doc = convert(&wrap(
            "<absatz style=\"padding-left:38px;text-indent:-38px;\">1.1.<tab/>Erster Punkt.</absatz>\
             <absatz style=\"padding-left:62px;text-indent:-24px;\">-<tab/>Gegen die Beschuldigte.</absatz>\
             <absatz style=\"padding-left:62px;text-indent:-24px;\">-<tab/>Mitgewirkt.</absatz>\
             <absatz style=\"padding-left:86px;text-indent:-24px;\"><tab/>Ohne Zeichen.</absatz>\
             <absatz style=\"padding-left:86px;text-indent:-24px;\">\u{f0b7}<tab/>Symbol.</absatz>",
        ));
        assert_eq!(
            doc.export_to_markdown(),
            "1.1. Erster Punkt.\n\n- Gegen die Beschuldigte.\n- Mitgewirkt.\n- Ohne Zeichen.\n- Symbol.\n"
        );
    }

    /// A table: the layout row dropped, spans honoured.
    #[test]
    fn tables_drop_the_layout_row() {
        let doc = convert(&wrap(
            "<table><tr><td><absatz typ=\"tabtext\"><b>Geldstrafe von Euro</b></absatz></td>\
             <td><absatz><b>Gemäß</b></absatz></td></tr>\
             <tr><td><absatz>€ 870,-</absatz></td><td><absatz>Art. 83 Abs. 4</absatz></td></tr>\
             <tr><td colspan=\"2\"><absatz>Summe</absatz></td></tr>\
             <tr height=\"0\"><td width=\"310\" /><td width=\"340\" /></tr></table>",
        ));
        let json: serde_json::Value = serde_json::from_str(&doc.export_to_json()).unwrap();
        let data = &json["tables"][0]["data"];
        assert_eq!(data["num_rows"], 3);
        assert_eq!(data["num_cols"], 2);
        assert_eq!(data["table_cells"][4]["col_span"], 2);
        let md = doc.export_to_markdown();
        assert!(md.contains("| € 870,-"), "{md}");
    }

    /// Pseudonym stars stay text: escaped in the Markdown, literal in the
    /// plain text and the JSON.
    #[test]
    fn pseudonym_stars_are_text() {
        let doc = convert(&wrap(
            "<absatz><u><b>Beschuldigte: A*** GmbH (FN *0*271*h)</b></u></absatz>\
             <absatz>mit Sitz in **** L***berg</absatz>",
        ));
        assert_eq!(
            doc.export_to_markdown(),
            "**Beschuldigte: A\\*\\*\\* GmbH (FN \\*0\\*271\\*h)**\n\n\
             mit Sitz in \\*\\*\\*\\* L\\*\\*\\*berg\n"
        );
        assert_eq!(
            doc.export_to_text(),
            "Beschuldigte: A*** GmbH (FN *0*271*h)\n\nmit Sitz in **** L***berg"
        );
        let json: serde_json::Value = serde_json::from_str(&doc.export_to_json()).unwrap();
        assert_eq!(json["texts"][1]["text"], "mit Sitz in **** L***berg");
    }

    /// A bare `.xml` (and bytes with no name to go by) reach the RIS reader
    /// by their root, no longer one `xml` code block; `xml_ris` names the
    /// format.
    #[test]
    fn bare_xml_routes_to_ris() {
        let xml = wrap("<ueberschrift typ=\"titel\">Geschäftszahl</ueberschrift><absatz>2025-0.699.550</absatz>");
        for src in [
            SourceDocument::from_bytes("ris-min.xml", InputFormat::Xml, xml.clone().into_bytes()),
            SourceDocument::from_named_bytes("stdin", xml.clone().into_bytes(), None).unwrap(),
        ] {
            let md = crate::DocumentConverter::new()
                .convert(src)
                .expect("convert")
                .document
                .export_to_markdown();
            assert_eq!(md, "## Geschäftszahl\n\n2025-0.699.550\n");
        }
        assert_eq!(InputFormat::from_id("xml_ris"), Some(InputFormat::XmlRis));
        assert_eq!(InputFormat::XmlRis.as_str(), "xml_ris");
    }

    #[test]
    fn sniffs_the_risdok_root() {
        assert!(looks_like_ris(
            "<?xml version=\"1.0\"?><risdok stylesheet=\"x.xsl\" xmlns=\"http://www.bka.gv.at\">"
        ));
        assert!(looks_like_ris("<risdok><nutzdaten/></risdok>"));
        assert!(!looks_like_ris("<risdok xmlns=\"urn:other\">"));
        assert!(!looks_like_ris("<article><risdok/></article>"));
    }
}
