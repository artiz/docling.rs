//! LaTeX backend — a port of docling's `LatexDocumentBackend` (#466).
//!
//! Upstream parses the source with pylatexenc and walks the node stream
//! with a handful of handler mixins (`handlers/macros.py`,
//! `handlers/environments.py`, `handlers/math.py`, `utils/text.py`,
//! `utils/table.py`); [`super::latex_walker`] is the parser port and this
//! module the handlers, function for function, including the corners that
//! shape upstream's groundtruth — a text buffer flushed by structural macros
//! and environments, `\input` files parsed in place, `\newcommand` bodies
//! expanded by text, a `tabular` read cell by cell with `\multicolumn` and
//! `\multirow` looked up at absolute source offsets (upstream slices the
//! environment's own text with the node's document position, so the lookup
//! usually finds nothing and the macro's groups become cell text), and the
//! TikZ / Tectonic rendering left out (no engine here: a `tikzpicture`
//! is a payload-less picture, where upstream without Tectonic keeps the
//! source as the picture's `meta.code`).
//!
//! What upstream embeds as picture payloads is reproduced for raster
//! figures (decoded and re-encoded as PNG, as `ImageRef.from_pil` does); a
//! PDF figure, which upstream renders with pypdfium2, stays a payload-less
//! picture here.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use regex::Regex;

use super::latex_walker::{Kind, Node as LNode, Walker};
use super::markdown::escape_text;
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;
use docling_core::tree::{ItemTree, ListMeta, TreeKind};
use docling_core::{DoclingDocument, Node, PictureImage, Table, TableCell};

pub struct LatexBackend;

impl DeclarativeBackend for LatexBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let text = decode_latex(&source.bytes);
        let mut conv = Converter {
            base_dir: source
                .path
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf),
            custom_macros: Vec::new(),
            custom_macro_num_args: HashMap::new(),
            input_stack: HashSet::new(),
            labels: HashSet::new(),
            doc: Builder::default(),
        };
        conv.run(&text);
        let mut doc = DoclingDocument::new(&source.name);
        let (nodes, tree) = conv.doc.finish();
        doc.nodes = nodes;
        doc.tree = Some(tree);
        Ok(doc)
    }
}

/// `decode_latex_content`: UTF-8, else latin-1 (which never fails, so
/// cp1252 is never reached).
fn decode_latex(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

// ---------------------------------------------------------------------------
// Constants (`constants.py`)

const MACROS_NEWCOMMAND: &[&str] = &["newcommand", "renewcommand", "providecommand"];
const MACROS_PREAMBLE_METADATA: &[&str] = &["title", "author", "date"];
const MACROS_INLINE_VERBATIM: &[&str] = &["%", "$", "&", "#", "_", "{", "}", "~"];
const MACROS_TEXT_FORMATTING: &[&str] = &["textbf", "textit", "emph", "texttt", "underline"];
const MACROS_CITATION: &[&str] = &["cite", "citep", "citet", "ref", "eqref"];
const MACROS_COLOR: &[&str] = &["color", "definecolor", "colorlet"];
const MACROS_COLOR_INLINE: &[&str] = &["textcolor", "colorbox"];
const MACROS_STRUCTURAL: &[&str] = &[
    "section",
    "subsection",
    "subsubsection",
    "chapter",
    "part",
    "paragraph",
    "subparagraph",
    "caption",
    "label",
    "includegraphics",
    "bibliography",
    "title",
    "author",
    "maketitle",
    "footnote",
    "marginpar",
    "textsc",
    "textsf",
    "textrm",
    "textnormal",
    "mbox",
    "href",
    "newline",
    "hfill",
    "break",
    "centering",
    "textcolor",
    "colorbox",
    "item",
    "input",
    "include",
];
const MACROS_HEADING: &[&str] = &[
    "part",
    "chapter",
    "section",
    "subsection",
    "subsubsection",
    "paragraph",
    "subparagraph",
];
const MACROS_TEXT_STYLE: &[&str] = &["textsc", "textsf", "textrm", "textnormal", "mbox"];
const MACROS_IGNORED: &[&str] = &[
    "documentclass",
    "usepackage",
    "geometry",
    "hypersetup",
    "lstset",
    "bibliographystyle",
    "newcommand",
    "renewcommand",
    "def",
    "let",
    "edef",
    "gdef",
    "xdef",
    "newenvironment",
    "renewenvironment",
    "DeclareMathOperator",
    "DeclareMathSymbol",
    "setlength",
    "setcounter",
    "addtolength",
    "color",
    "definecolor",
    "colorlet",
    "AtBeginDocument",
    "AtEndDocument",
    "newlength",
    "newcounter",
    "newif",
    "providecommand",
    "DeclareOption",
    "RequirePackage",
    "ProvidesPackage",
    "LoadClass",
    "makeatletter",
    "makeatother",
    "NeedsTeXFormat",
    "ProvidesClass",
    "DeclareRobustCommand",
    "newtheorem",
    "theoremstyle",
    "newtheoremstyle",
    "documentstyle",
    "pagestyle",
    "thispagestyle",
    "pagenumbering",
    "tableofcontents",
    "listoffigures",
    "listoftables",
    "appendix",
    "cleardoublepage",
    "clearpage",
    "newpage",
    "markboth",
    "markright",
    "lhead",
    "rhead",
    "cfoot",
    "hyphenation",
    "overfullrule",
    "protect",
];
const MACROS_SPACING: &[&str] = &[
    "newline",
    "hfill",
    "break",
    "centering",
    "noindent",
    "par",
    "smallskip",
    "medskip",
    "bigskip",
    "vfill",
    "vskip",
    "hskip",
    "vspace",
    "hspace",
];
const MACROS_ESCAPED: &[&str] = &["&", "%", "$", "#", "_", "{", "}"];
const ENV_MATH_DISPLAY_PREFIXES: &[&str] = &[
    "$$",
    "\\[",
    "\\begin{equation}",
    "\\begin{align}",
    "\\begin{gather}",
    "\\begin{displaymath}",
];
const ENV_MATH_CLEAN: &[&str] = &[
    "equation",
    "equation*",
    "displaymath",
    "math",
    "eqnarray",
    "eqnarray*",
    "dmath",
    "dmath*",
];
const ENV_MATH: &[&str] = &[
    "equation",
    "align",
    "gather",
    "multline",
    "flalign",
    "alignat",
    "displaymath",
    "eqnarray",
    "dmath",
    "dgroup",
    "darray",
];
const ENV_THEOREM: &[&str] = &[
    "theorem",
    "lemma",
    "corollary",
    "proposition",
    "definition",
    "remark",
    "example",
    "conjecture",
];
const ENV_LIST: &[&str] = &["itemize", "enumerate", "description"];
const ENV_QUOTE: &[&str] = &["quote", "quotation", "verse"];
const TABLE_MACROS_RULE: &[&str] = &[
    "hline",
    "cline",
    "toprule",
    "midrule",
    "bottomrule",
    "cmidrule",
    "specialrule",
];
const TABLE_MACROS_IGNORE: &[&str] = &[
    "rule",
    "vspace",
    "hspace",
    "vskip",
    "hskip",
    "smallskip",
    "medskip",
    "bigskip",
    "strut",
    "phantom",
    "hphantom",
    "vphantom",
    "noalign",
    // longtable's repeating header/footer markers are not cell content
    // (docling#4325).
    "endhead",
    "endfirsthead",
    "endfoot",
    "endlastfoot",
];

/// The `\input` nesting upstream allows.
const MAX_INPUT_DEPTH: usize = 10;

// ---------------------------------------------------------------------------
// The document builder: docling's `DoclingDocument` add_* calls, recorded as
// both the item tree (JSON) and the flat nodes (Markdown, DocLang, LaTeX).

#[derive(Default)]
struct Builder {
    tree: ItemTree,
    /// One flat node per tree item, `None` for items with no rendering of
    /// their own (groups, a caption adopted by its picture).
    flat: Vec<Option<Node>>,
    /// List groups that have not received their first item yet.
    fresh_lists: HashSet<usize>,
}

impl Builder {
    fn add_text(&mut self, parent: Option<usize>, label: &str, text: &str) -> usize {
        let list = (label == "list_item").then(ListMeta::default);
        let id = self.tree.add(
            parent,
            None,
            TreeKind::Text {
                label: label.into(),
                text: text.into(),
                orig: None,
                formatting: None,
                hyperlink: None,
                level: None,
                list,
            },
        );
        let node = match label {
            "title" => Node::Heading {
                level: 1,
                text: escape_text(text),
            },
            "formula" => Node::Formula {
                latex: text.into(),
                orig: text.into(),
                location: None,
            },
            "list_item" => {
                let first = parent.is_some_and(|p| self.fresh_lists.remove(&p));
                Node::ListItem {
                    ordered: false,
                    number: 0,
                    first_in_list: first,
                    text: escape_text(text),
                    level: 0,
                    marker: None,
                    location: None,
                    dclx: None,
                    href: None,
                    layer: None,
                }
            }
            _ => Node::Paragraph {
                text: escape_text(text),
            },
        };
        self.flat.push(Some(node));
        id
    }

    fn add_code(&mut self, parent: Option<usize>, text: &str) {
        self.tree.add(
            parent,
            None,
            TreeKind::Code {
                text: text.into(),
                orig: None,
                language: None,
                formatting: None,
                hyperlink: None,
            },
        );
        self.flat.push(Some(Node::Code {
            language: None,
            text: text.into(),
            orig: None,
            pretty: None,
        }));
    }

    fn add_heading(&mut self, parent: Option<usize>, text: &str, level: u8) {
        self.tree.add(
            parent,
            None,
            TreeKind::Text {
                label: "section_header".into(),
                text: text.into(),
                orig: None,
                formatting: None,
                hyperlink: None,
                level: Some(level),
                list: None,
            },
        );
        self.flat.push(Some(Node::Heading {
            level: level.saturating_add(1),
            text: escape_text(text),
        }));
    }

    fn add_group(&mut self, parent: Option<usize>, name: &str, label: &str) -> usize {
        let id = self.tree.add(
            parent,
            None,
            TreeKind::Group {
                label: label.into(),
                name: name.into(),
            },
        );
        if label == "list" {
            self.fresh_lists.insert(id);
        }
        self.flat.push(None);
        id
    }

    fn add_table(&mut self, parent: Option<usize>, table: Table) {
        self.tree.add(
            parent,
            None,
            TreeKind::Table {
                table: table.clone(),
                rich_cells: Vec::new(),
                captions: Vec::new(),
            },
        );
        self.flat.push(Some(Node::Table(table)));
    }

    /// `add_picture(parent, caption, image)`: the caption item, created
    /// beforehand on the body, is referenced by the picture and rendered as
    /// its caption (docling's Markdown serializes a picture's captions with
    /// it and skips them where they sit).
    fn add_picture(
        &mut self,
        parent: Option<usize>,
        caption: Option<usize>,
        image: Option<PictureImage>,
        dpi: Option<u32>,
    ) {
        let caption_text =
            caption.and_then(|c| match self.flat.get_mut(c).and_then(Option::take) {
                Some(Node::Paragraph { text }) => Some(text),
                _ => None,
            });
        self.tree.add(
            parent,
            None,
            TreeKind::Picture {
                captions: caption.into_iter().collect(),
                image: image.clone(),
                classification: None,
                description: None,
                confidence: None,
                chart: None,
                dpi,
            },
        );
        self.flat.push(Some(Node::Picture {
            caption: caption_text,
            caption_href: None,
            image,
            classification: None,
            description: None,
            caption_parent: Default::default(),
            caption_location: None,
        }));
    }

    /// The flat node list in tree order, groups nested.
    fn finish(mut self) -> (Vec<Node>, ItemTree) {
        fn collect(b: &mut Builder, id: usize) -> Option<Node> {
            let children: Vec<usize> = b.tree.items[id].children.clone();
            if let TreeKind::Group { label, name } = &b.tree.items[id].kind {
                let (label, name) = (label.clone(), name.clone());
                let children: Vec<Node> =
                    children.into_iter().filter_map(|c| collect(b, c)).collect();
                if children.is_empty() {
                    return None;
                }
                return Some(Node::Group {
                    label,
                    name: Some(name),
                    layer: None,
                    children,
                });
            }
            b.flat[id].take()
        }
        let body = self.tree.body.clone();
        let nodes = body
            .into_iter()
            .filter_map(|id| collect(&mut self, id))
            .collect();
        (nodes, self.tree)
    }
}

// ---------------------------------------------------------------------------
// The converter (`LatexDocumentBackend` + mixins)

struct Converter {
    base_dir: Option<PathBuf>,
    /// `\newcommand` definitions in registration order (Python's dict order),
    /// name → body.
    custom_macros: Vec<(String, String)>,
    custom_macro_num_args: HashMap<String, usize>,
    input_stack: HashSet<PathBuf>,
    labels: HashSet<String>,
    doc: Builder,
}

/// One node with the source it was parsed from — cell content assembled from
/// a `\multicolumn` argument is re-parsed from its own string.
type Src<'a> = (&'a LNode, &'a str);

impl Converter {
    fn run(&mut self, latex_text: &str) {
        let preprocessed = preprocess_custom_macros(latex_text);
        let walker = Walker::new(&preprocessed);
        let nodes = walker.parse();
        let src = preprocessed.as_str();
        self.extract_custom_macros(&nodes, src, 0);
        self.extract_preamble_metadata(&nodes, src, 0);
        match find_document_env(&nodes, 0) {
            Some(doc_node) => {
                if let Some(list) = doc_node.nodelist() {
                    self.process_nodes(list, src, None, None);
                }
            }
            None => self.process_nodes(&nodes, src, None, None),
        }
    }

    // -- custom macros -------------------------------------------------------

    fn custom_macro(&self, name: &str) -> Option<&str> {
        self.custom_macros
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, def)| def.as_str())
    }

    fn set_custom_macro(&mut self, name: &str, def: String, num_args: usize) {
        match self.custom_macros.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = def,
            None => self.custom_macros.push((name.to_string(), def)),
        }
        self.custom_macro_num_args
            .insert(name.to_string(), num_args);
    }

    fn extract_custom_macros(&mut self, nodes: &[LNode], src: &str, depth: usize) {
        if depth > 10 {
            return;
        }
        for node in nodes {
            if let Kind::Macro {
                name,
                args: Some(args),
                ..
            } = &node.kind
            {
                if MACROS_NEWCOMMAND.contains(&name.as_str()) && !args.argnlist.is_empty() {
                    let list = &args.argnlist;
                    let name_arg = list.get(1).and_then(Option::as_ref);
                    let num_args_arg = list.get(2).and_then(Option::as_ref);
                    let def_arg = list.iter().rev().find_map(Option::as_ref);
                    if let (Some(name_arg), Some(def_arg)) = (name_arg, def_arg) {
                        if !std::ptr::eq(name_arg, def_arg) {
                            let mut macro_name = name_arg
                                .verbatim(src)
                                .trim_matches(|c| "{} \n\t".contains(c))
                                .to_string();
                            if let Some(rest) = macro_name.strip_prefix('\\') {
                                macro_name = rest.to_string();
                            }
                            let macro_def = if def_arg.nodelist().is_some() {
                                let v = def_arg.verbatim(src);
                                match v.strip_prefix('{').and_then(|v| v.strip_suffix('}')) {
                                    Some(inner) => inner.to_string(),
                                    None => v.to_string(),
                                }
                            } else {
                                def_arg
                                    .verbatim(src)
                                    .trim_matches(|c| "{} ".contains(c))
                                    .to_string()
                            };
                            if !macro_name.is_empty() {
                                let n = parse_custom_macro_num_args(num_args_arg, src);
                                self.set_custom_macro(&macro_name, macro_def, n);
                            }
                        }
                    }
                }
            }
            if let Some(list) = node.nodelist().filter(|l| !l.is_empty()) {
                self.extract_custom_macros(list, src, depth + 1);
            }
            if let Some(args) = node.args() {
                for arg in args.argnlist.iter().flatten() {
                    if let Some(list) = arg.nodelist().filter(|l| !l.is_empty()) {
                        self.extract_custom_macros(list, src, depth + 1);
                    }
                }
            }
        }
    }

    /// `_extract_preamble_metadata`: `\title` / `\author` / `\date` seen before
    /// the `document` environment, at any depth.
    fn extract_preamble_metadata(&mut self, nodes: &[LNode], src: &str, depth: usize) {
        if depth > 10 {
            return;
        }
        for node in nodes {
            if node.env_name() == Some("document") {
                return;
            }
            if let Some(name) = node.macro_name() {
                if MACROS_PREAMBLE_METADATA.contains(&name) {
                    let text = self.extract_macro_arg(node, src);
                    if !text.is_empty() {
                        let label = if name == "title" { "title" } else { "text" };
                        self.doc.add_text(None, label, &text);
                    }
                }
            }
            if let Some(list) = node.nodelist().filter(|l| !l.is_empty()) {
                self.extract_preamble_metadata(list, src, depth + 1);
            }
            if let Some(args) = node.args() {
                for arg in args.argnlist.iter().flatten() {
                    if let Some(list) = arg.nodelist().filter(|l| !l.is_empty()) {
                        self.extract_preamble_metadata(list, src, depth + 1);
                    }
                }
            }
        }
    }

    // -- the node walk ---------------------------------------------------------

    /// `_process_nodes`: the text buffer collects chars, inline macros and
    /// inline math; environments, structural macros and paragraph breaks
    /// flush it as one text item.
    fn process_nodes(
        &mut self,
        nodes: &[LNode],
        src: &str,
        parent: Option<usize>,
        text_label: Option<&str>,
    ) {
        let mut buffer: Vec<String> = Vec::new();
        let mut idx = 0;
        while idx < nodes.len() {
            let node = &nodes[idx];
            let mut consumed = 0;
            match &node.kind {
                Kind::Chars(text) => self.process_chars_node(text, parent, text_label, &mut buffer),
                Kind::Macro { .. } => {
                    consumed = self.process_macro_node_inline(
                        node,
                        src,
                        parent,
                        text_label,
                        &mut buffer,
                        &nodes[idx + 1..],
                    );
                }
                Kind::Env { .. } => {
                    self.flush(&mut buffer, parent, text_label);
                    self.process_environment(node, src, parent, text_label);
                }
                Kind::Math { .. } => {
                    self.process_math_node(node, src, parent, text_label, &mut buffer)
                }
                Kind::Group { nodelist, .. } => {
                    if !nodelist.is_empty() && self.is_text_only_group(node) {
                        let group_text = self.nodes_to_text(nodelist, src);
                        if !group_text.is_empty() {
                            buffer.push(group_text);
                        }
                    } else if !nodelist.is_empty() {
                        self.flush(&mut buffer, parent, text_label);
                        self.process_nodes(nodelist, src, parent, text_label);
                    }
                }
                // pylatexenc's specials and comments have no handler upstream.
                Kind::Specials(_) | Kind::Comment { .. } => {}
            }
            idx += 1 + consumed;
        }
        self.flush(&mut buffer, parent, text_label);
    }

    fn flush(&mut self, buffer: &mut Vec<String>, parent: Option<usize>, text_label: Option<&str>) {
        if buffer.is_empty() {
            return;
        }
        let combined = buffer.concat();
        let combined = combined.trim();
        if !combined.is_empty() {
            self.doc
                .add_text(parent, text_label.unwrap_or("text"), combined);
        }
        buffer.clear();
    }

    /// `_process_chars_node` (docling#4340): a paragraph break inside a chars
    /// node flushes the buffer with the first part, adds every middle part as
    /// its own `paragraph`, and leaves the last part in the buffer.
    fn process_chars_node(
        &mut self,
        text: &str,
        parent: Option<usize>,
        text_label: Option<&str>,
        buffer: &mut Vec<String>,
    ) {
        if text.contains("\n\n") {
            let parts: Vec<&str> = text.split("\n\n").collect();
            buffer.push(parts[0].to_string());
            self.flush(buffer, parent, text_label);
            for part in &parts[1..parts.len() - 1] {
                let stripped = part.trim();
                if !stripped.is_empty() {
                    self.doc
                        .add_text(parent, text_label.unwrap_or("paragraph"), stripped);
                }
            }
            buffer.push(parts[parts.len() - 1].to_string());
        } else {
            buffer.push(text.to_string());
        }
    }

    fn process_math_node(
        &mut self,
        node: &LNode,
        src: &str,
        parent: Option<usize>,
        text_label: Option<&str>,
        buffer: &mut Vec<String>,
    ) {
        let Kind::Math { display, .. } = &node.kind else {
            return;
        };
        let verbatim = node.verbatim(src);
        let is_display = *display
            || ENV_MATH_DISPLAY_PREFIXES
                .iter()
                .any(|p| verbatim.starts_with(p));
        if is_display {
            self.flush(buffer, parent, text_label);
            let math = self.clean_math(verbatim, "display");
            self.doc.add_text(parent, "formula", &math);
        } else {
            buffer.push(self.expand_macros(verbatim));
        }
    }

    /// `_process_macro_node_inline`; returns how many following nodes a
    /// custom macro's arguments consumed.
    fn process_macro_node_inline(
        &mut self,
        node: &LNode,
        src: &str,
        parent: Option<usize>,
        text_label: Option<&str>,
        buffer: &mut Vec<String>,
        following: &[LNode],
    ) -> usize {
        let Kind::Macro { name, args, .. } = &node.kind else {
            return 0;
        };
        let name = name.as_str();
        if MACROS_INLINE_VERBATIM.contains(&name) {
            buffer.push(if name == "~" { " ".into() } else { name.into() });
        } else if name == " " {
            buffer.push(" ".into());
        } else if MACROS_TEXT_FORMATTING.contains(&name) {
            let text = self.extract_macro_arg(node, src);
            if !text.is_empty() {
                buffer.push(text);
            }
        } else if self.custom_macro(name).is_some() {
            let (expansion, consumed) = self.expand_custom_macro_invocation(node, following, src);
            if !expansion.is_empty() {
                if self.custom_macro_num_args.get(name).copied().unwrap_or(0) > 0 {
                    buffer.push(self.parse_latex_fragment_to_text(&expansion));
                } else {
                    buffer.push(expansion);
                }
            }
            return consumed;
        } else if MACROS_CITATION.contains(&name) {
            let arg = self.extract_macro_arg(node, src);
            if !arg.is_empty() {
                buffer.push(format!("[{arg}]"));
            }
        } else if name == "url" {
            let url = self.extract_macro_arg(node, src);
            if !url.is_empty() {
                buffer.push(url);
            }
        } else if MACROS_COLOR.contains(&name) {
        } else if MACROS_TEXT_STYLE.contains(&name) {
            let text = self.extract_macro_arg(node, src);
            if !text.is_empty() {
                buffer.push(text);
            }
        } else if MACROS_COLOR_INLINE.contains(&name) {
            // The text is always the last argument; the color is skipped.
            if let Some(Some(arg)) = args.as_ref().and_then(|a| a.argnlist.last()) {
                if let Some(list) = arg.nodelist() {
                    let text = self.nodes_to_text(list, src);
                    if !text.is_empty() {
                        buffer.push(text);
                    }
                }
            }
        } else if MACROS_STRUCTURAL.contains(&name) {
            self.flush(buffer, parent, text_label);
            self.process_macro(node, src, parent, text_label);
        } else if MACROS_SPACING.contains(&name) || MACROS_IGNORED.contains(&name) {
            // Discarded with their arguments (`\vspace{-1mm}` emits nothing).
        } else if args.as_ref().is_some_and(|a| !a.argnlist.is_empty()) {
            let inline = self.extract_all_macro_args_inline(node, src);
            if !inline.is_empty() {
                buffer.push(inline);
            }
        }
        0
    }

    /// `_process_macro`: the structural macros, each its own item.
    fn process_macro(
        &mut self,
        node: &LNode,
        src: &str,
        parent: Option<usize>,
        text_label: Option<&str>,
    ) {
        let Kind::Macro { name, args, .. } = &node.kind else {
            return;
        };
        let name = name.as_str();
        let text_label = text_label.unwrap_or("text");
        if MACROS_HEADING.contains(&name) {
            let title = self.extract_macro_arg(node, src);
            if !title.is_empty() {
                self.doc.add_heading(parent, &title, heading_level(name));
            }
        } else if name == "title" {
            let title = self.extract_macro_arg(node, src);
            if !title.is_empty() {
                self.doc.add_text(parent, "title", &title);
            }
        } else if name == "author" || name == "date" {
            let meta = self.extract_macro_arg(node, src);
            if !meta.is_empty() {
                self.doc.add_text(parent, "text", &meta);
            }
        } else if matches!(name, "thanks" | "maketitle" | "\\" | "item")
            || MACROS_IGNORED.contains(&name)
        {
            // Upstream's no-op branches, none of whose names another branch
            // claims, gathered in one place.
        } else if MACROS_TEXT_STYLE.contains(&name) {
            if let Some(Some(arg)) = args.as_ref().and_then(|a| a.argnlist.last()) {
                if let Some(list) = arg.nodelist() {
                    self.process_nodes(list, src, parent, Some(text_label));
                }
            }
        } else if MACROS_CITATION.contains(&name) {
            let arg = self.extract_macro_arg(node, src);
            if !arg.is_empty() {
                self.doc.add_text(parent, "reference", &format!("[{arg}]"));
            }
        } else if name == "url" {
            let url = self.extract_macro_arg(node, src);
            if !url.is_empty() {
                self.doc.add_text(parent, "reference", &url);
            }
        } else if name == "label" {
            let label = self.extract_macro_arg(node, src);
            if !label.is_empty() {
                self.labels.insert(label);
            }
        } else if name == "caption" {
            let caption = self.extract_macro_arg(node, src);
            if !caption.is_empty() {
                self.doc.add_text(parent, "caption", &caption);
            }
        } else if name == "footnote" || name == "marginpar" {
            let text = self.extract_macro_arg(node, src);
            if !text.is_empty() {
                self.doc.add_text(parent, "footnote", &text);
            }
        } else if name == "includegraphics" {
            let img_path = self.extract_macro_arg(node, src);
            if !img_path.is_empty() {
                let (image, dpi) = self.load_image(&img_path);
                let caption = self
                    .doc
                    .add_text(None, "caption", &format!("Image: {img_path}"));
                self.doc.add_picture(parent, Some(caption), image, dpi);
            }
        } else if name == "input" || name == "include" {
            let filepath = self.extract_macro_arg(node, src);
            if !filepath.is_empty() {
                self.process_input(&filepath, parent, Some(text_label));
            }
        } else if MACROS_ESCAPED.contains(&name) {
            self.doc.add_text(parent, text_label, name);
        } else if name == "href" {
            let Some(a) = args.as_ref().filter(|a| a.argnlist.len() >= 2) else {
                return;
            };
            let arg_text = |arg: &Option<LNode>| -> String {
                match arg {
                    Some(n) => match n.nodelist() {
                        Some(list) => self.nodes_to_text(list, src),
                        None => n
                            .verbatim(src)
                            .trim_matches(|c| "{} ".contains(c))
                            .to_string(),
                    },
                    None => String::new(),
                }
            };
            let url = arg_text(&a.argnlist[0]);
            let display = arg_text(&a.argnlist[1]);
            let link = match (url.is_empty(), display.is_empty()) {
                (false, false) => format!("[{display}]({url})"),
                (false, true) => url,
                (true, false) => display,
                (true, true) => String::new(),
            };
            if !link.is_empty() {
                self.doc.add_text(parent, "reference", &link);
            }
        } else if MACROS_SPACING.contains(&name) {
            if name == "newline" {
                self.doc.add_text(parent, text_label, "\n");
            }
        } else if name == "textcolor" || name == "colorbox" {
            if let Some(a) = args {
                if let Some(arg) = a
                    .argnlist
                    .iter()
                    .rev()
                    .flatten()
                    .find(|n| n.nodelist().is_some())
                {
                    if let Some(list) = arg.nodelist() {
                        self.process_nodes(list, src, parent, Some(text_label));
                    }
                }
            }
        } else if let Some(a) = args {
            for arg in a.argnlist.iter().flatten() {
                if let Some(list) = arg.nodelist() {
                    self.process_nodes(list, src, parent, Some(text_label));
                }
            }
        }
    }

    /// `\input` / `\include`: the file parsed in place, relative to the
    /// source file, `.tex` implied, kept inside the source's directory.
    fn process_input(&mut self, filepath: &str, parent: Option<usize>, text_label: Option<&str>) {
        let Some(base_dir) = self.base_dir.clone() else {
            return;
        };
        let mut input_path = base_dir.join(filepath);
        if input_path.extension().is_none() {
            input_path.set_extension("tex");
        }
        let Some(resolved) = within(&base_dir, &input_path) else {
            return;
        };
        if self.input_stack.contains(&resolved) || self.input_stack.len() >= MAX_INPUT_DEPTH {
            return;
        }
        let Ok(content) = std::fs::read_to_string(&input_path) else {
            return;
        };
        self.input_stack.insert(resolved.clone());
        let walker = Walker::new(&content);
        let nodes = walker.parse();
        self.process_nodes(&nodes, &content, parent, text_label);
        self.input_stack.remove(&resolved);
    }

    /// `\includegraphics`: a raster file becomes the picture's PNG payload
    /// (the `dpi` its metadata records, 72 when none); a PDF — rendered by
    /// pypdfium2 upstream — or a missing file leaves the picture without one.
    fn load_image(&self, img_path: &str) -> (Option<PictureImage>, Option<u32>) {
        let Some(base_dir) = &self.base_dir else {
            return (None, None);
        };
        let full = base_dir.join(img_path);
        if within(base_dir, &full).is_none() || !full.is_file() {
            return (None, None);
        }
        if full
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            return (None, None);
        }
        let Ok(data) = std::fs::read(&full) else {
            return (None, None);
        };
        let dpi = super::ooxml::image_dpi(&data).map_or(72, |d| d as u32);
        let Ok(reader) = image::ImageReader::new(std::io::Cursor::new(&data)).with_guessed_format()
        else {
            return (None, None);
        };
        let Ok(decoded) = reader.decode() else {
            return (None, None);
        };
        let mut png = std::io::Cursor::new(Vec::new());
        if decoded.write_to(&mut png, image::ImageFormat::Png).is_err() {
            return (None, None);
        }
        (
            Some(PictureImage {
                dpi: PictureImage::DEFAULT_DPI,
                mimetype: "image/png".into(),
                width: decoded.width(),
                height: decoded.height(),
                data: png.into_inner(),
            }),
            Some(dpi),
        )
    }

    // -- environments ----------------------------------------------------------

    fn process_environment(
        &mut self,
        node: &LNode,
        src: &str,
        parent: Option<usize>,
        text_label: Option<&str>,
    ) {
        let Kind::Env { name, nodelist, .. } = &node.kind else {
            return;
        };
        let name = name.as_str();
        let unstarred = name.replace('*', "");
        match name {
            "document" => self.process_nodes(nodelist, src, parent, text_label),
            "abstract" => {
                self.doc.add_heading(parent, "Abstract", 1);
                self.process_nodes(nodelist, src, parent, text_label);
            }
            _ if ENV_MATH.contains(&unstarred.as_str()) || name == "math" => {
                let math = self.clean_math(node.verbatim(src), name);
                self.doc.add_text(parent, "formula", &math);
            }
            "subequations" => self.process_nodes(nodelist, src, parent, text_label),
            _ if ENV_THEOREM.contains(&unstarred.as_str()) => {
                let title = capitalize(&unstarred);
                self.doc.add_text(parent, "text", &format!("**{title}.**"));
                self.process_nodes(nodelist, src, parent, text_label);
            }
            "proof" => {
                self.doc.add_text(parent, "text", "*Proof.*");
                self.process_nodes(nodelist, src, parent, text_label);
                let body = node.verbatim(src);
                if !body.contains("\\qed") && !body.contains("\\qedsymbol") {
                    self.doc.add_text(parent, "text", "\u{25fb}");
                }
            }
            _ if ENV_QUOTE.contains(&name) => self.process_nodes(nodelist, src, parent, text_label),
            _ if ENV_LIST.contains(&name) => self.process_list(nodelist, src, parent, text_label),
            "tabular" | "tabular*" | "tabularx" | "longtable" => {
                if let Some(table) = self.parse_table(node, src) {
                    self.doc.add_table(parent, table);
                }
            }
            "table" | "table*" => self.process_nodes(nodelist, src, parent, text_label),
            "figure" | "figure*" => {
                let group = self.doc.add_group(parent, "figure", "section");
                self.process_nodes(nodelist, src, Some(group), text_label);
            }
            "tikzpicture" => self.process_tikzpicture(node, src, parent, text_label),
            "verbatim" | "lstlisting" | "minted" => {
                let code = extract_verbatim_content(node.verbatim(src), name);
                self.doc.add_code(parent, &code);
            }
            "thebibliography" => {
                self.doc.add_heading(parent, "References", 1);
                self.process_bibliography(nodelist, src, parent);
            }
            "filecontents" | "filecontents*" => {}
            _ => self.process_nodes(nodelist, src, parent, text_label),
        }
    }

    /// `_process_tikzpicture` without an engine: the environment's source
    /// becomes a picture carrying it as TikZ code (upstream's fallback), or
    /// its content is walked when the source is not one self-contained
    /// `tikzpicture`.
    fn process_tikzpicture(
        &mut self,
        node: &LNode,
        src: &str,
        parent: Option<usize>,
        text_label: Option<&str>,
    ) {
        let raw = node.verbatim(src);
        let ends_ok = |s: &str| tikz_end_re().is_match(s);
        let Kind::Env { nodelist, .. } = &node.kind else {
            return;
        };
        if !ends_ok(raw) || !validate_tikz_nodelist(nodelist, src, 0) {
            self.process_nodes(nodelist, src, parent, text_label);
            return;
        }
        // Upstream attaches the source as `meta.code` (language `tikz`);
        // the picture item here carries no meta.
        let _ = raw;
        self.doc.add_picture(parent, None, None, None);
    }

    fn process_list(
        &mut self,
        nodelist: &[LNode],
        src: &str,
        parent: Option<usize>,
        text_label: Option<&str>,
    ) {
        let group = self.doc.add_group(parent, "list", "list");
        let mut items: Vec<Vec<&LNode>> = Vec::new();
        let mut current: Vec<&LNode> = Vec::new();
        for n in nodelist {
            if n.macro_name() == Some("item") {
                if !current.is_empty() {
                    items.push(std::mem::take(&mut current));
                }
                // The `\item` node itself stays when it has an argument list.
                if n.args().is_some_and(|a| !a.argnlist.is_empty()) {
                    current.push(n);
                }
            } else {
                current.push(n);
            }
        }
        if !current.is_empty() {
            items.push(current);
        }
        for item in items {
            let owned: Vec<LNode> = item.into_iter().cloned().collect();
            self.process_nodes(&owned, src, Some(group), Some("list_item"));
        }
        let _ = text_label;
    }

    fn process_bibliography(&mut self, nodelist: &[LNode], src: &str, parent: Option<usize>) {
        let group = self.doc.add_group(parent, "bibliography", "list");
        let mut items: Vec<(String, Vec<&LNode>)> = Vec::new();
        let mut current: Vec<&LNode> = Vec::new();
        let mut key = String::new();
        for n in nodelist {
            if n.macro_name() == Some("bibitem") {
                if !current.is_empty() {
                    items.push((std::mem::take(&mut key), std::mem::take(&mut current)));
                }
                key = self.extract_macro_arg(n, src);
            } else {
                current.push(n);
            }
        }
        if !current.is_empty() {
            items.push((key, current));
        }
        for (key, item) in items {
            if !key.is_empty() {
                self.doc
                    .add_text(Some(group), "list_item", &format!("[{key}] "));
            }
            let owned: Vec<LNode> = item.into_iter().cloned().collect();
            self.process_nodes(&owned, src, Some(group), Some("list_item"));
        }
    }

    // -- math ----------------------------------------------------------------

    fn clean_math(&self, latex: &str, env_name: &str) -> String {
        let mut s = latex.to_string();
        if ENV_MATH_CLEAN.contains(&env_name) {
            let begin = format!("\\begin{{{env_name}}}");
            let end = format!("\\end{{{env_name}}}");
            if let Some(start) = s.find(&begin) {
                if let Some(stop) = s[start + begin.len()..].find(&end) {
                    s = s[start + begin.len()..start + begin.len() + stop].to_string();
                }
            }
        }
        let mut s = s.trim().to_string();
        for (open, close) in [("$$", "$$"), ("$", "$"), ("\\[", "\\]"), ("\\(", "\\)")] {
            if s.starts_with(open) && s.ends_with(close) && s.len() >= open.len() + close.len() {
                s = s[open.len()..s.len() - close.len()].to_string();
                break;
            }
        }
        let s = label_re().replace_all(&s, "").into_owned();
        self.expand_macros(&s).trim().to_string()
    }

    /// `_expand_macros`: argument-less custom macros replaced by their bodies
    /// (`\\name(?![a-zA-Z])`, in registration order).
    fn expand_macros(&self, latex: &str) -> String {
        let mut s = latex.to_string();
        for (name, def) in &self.custom_macros {
            if self.custom_macro_num_args.get(name).copied().unwrap_or(0) > 0 {
                continue;
            }
            s = replace_macro_calls(&s, name, def);
        }
        s
    }

    fn expand_custom_macro_invocation(
        &self,
        node: &LNode,
        following: &[LNode],
        src: &str,
    ) -> (String, usize) {
        let Some(name) = node.macro_name() else {
            return (String::new(), 0);
        };
        let def = self.custom_macro(name).unwrap_or("").to_string();
        let expected = self.custom_macro_num_args.get(name).copied().unwrap_or(0);
        if expected == 0 {
            return (def, 0);
        }
        let mut values: Vec<String> = Vec::new();
        let mut consumed = 0;
        for next in following {
            if values.len() >= expected {
                break;
            }
            match &next.kind {
                Kind::Chars(c) if c.trim().is_empty() => consumed += 1,
                Kind::Group { nodelist, .. } => {
                    values.push(self.nodes_to_text(nodelist, src));
                    consumed += 1;
                }
                _ => break,
            }
        }
        if values.len() < expected {
            return (def, 0);
        }
        let mut expansion = def;
        for idx in (1..=values.len()).rev() {
            expansion = expansion.replace(&format!("#{idx}"), &values[idx - 1]);
        }
        (expansion, consumed)
    }

    fn parse_latex_fragment_to_text(&self, fragment: &str) -> String {
        let walker = Walker::new(fragment);
        let nodes = walker.parse();
        self.nodes_to_text(&nodes, fragment)
    }

    // -- text extraction (`utils/text.py`) -----------------------------------------

    fn extract_macro_arg(&self, node: &LNode, src: &str) -> String {
        let Some(Some(arg)) = node.args().and_then(|a| a.argnlist.last()) else {
            return String::new();
        };
        match arg.nodelist() {
            Some(list) => self.nodes_to_text(list, src),
            None => arg
                .verbatim(src)
                .trim_matches(|c| "{} ".contains(c))
                .to_string(),
        }
    }

    fn extract_all_macro_args_inline(&self, node: &LNode, src: &str) -> String {
        let Some(args) = node.args() else {
            return String::new();
        };
        let mut parts = Vec::new();
        for arg in args.argnlist.iter().flatten() {
            let text = match arg.nodelist() {
                Some(list) => self.nodes_to_text(list, src),
                None => arg
                    .verbatim(src)
                    .trim_matches(|c| "{} ".contains(c))
                    .to_string(),
            };
            if !text.is_empty() {
                parts.push(text);
            }
        }
        parts.join(" ")
    }

    fn nodes_to_text(&self, nodes: &[LNode], src: &str) -> String {
        let items: Vec<Src> = nodes.iter().map(|n| (n, src)).collect();
        self.nodes_to_text_mixed(&items)
    }

    /// `_nodes_to_text` over nodes that may come from different sources.
    fn nodes_to_text_mixed(&self, items: &[Src]) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut idx = 0;
        while idx < items.len() {
            let (node, src) = items[idx];
            let mut consumed = 0;
            match &node.kind {
                Kind::Chars(c) => parts.push(c.clone()),
                Kind::Group { nodelist, .. } => parts.push(self.nodes_to_text(nodelist, src)),
                Kind::Macro { .. } => {
                    let (text, n) = self.macro_node_to_text(node, &items[idx + 1..], src);
                    consumed = n;
                    if !text.is_empty() {
                        parts.push(text);
                    }
                }
                Kind::Math { .. } => parts.push(self.expand_macros(node.verbatim(src))),
                Kind::Env { name, nodelist, .. } => {
                    if matches!(name.as_str(), "equation" | "align" | "gather") {
                        parts.push(node.verbatim(src).to_string());
                    } else {
                        parts.push(self.nodes_to_text(nodelist, src));
                    }
                }
                Kind::Specials(_) | Kind::Comment { .. } => {}
            }
            idx += 1 + consumed;
        }
        let result = parts.concat();
        let result = spaces_re().replace_all(&result, " ");
        let result = blank_lines_re().replace_all(&result, "\n\n");
        result.trim().to_string()
    }

    fn macro_node_to_text(&self, node: &LNode, following: &[Src], src: &str) -> (String, usize) {
        let Kind::Macro { name, args, .. } = &node.kind else {
            return (String::new(), 0);
        };
        let name = name.as_str();
        if MACROS_TEXT_FORMATTING.contains(&name) || MACROS_TEXT_STYLE.contains(&name) {
            return (self.extract_macro_arg(node, src), 0);
        }
        if MACROS_COLOR_INLINE.contains(&name) {
            if let Some(Some(arg)) = args.as_ref().and_then(|a| a.argnlist.last()) {
                if let Some(list) = arg.nodelist() {
                    return (self.nodes_to_text(list, src), 0);
                }
            }
            return (String::new(), 0);
        }
        if MACROS_CITATION.contains(&name) {
            return (node.verbatim(src).to_string(), 0);
        }
        if name == "\\" {
            return ("\n".into(), 0);
        }
        if name == "~" {
            return (" ".into(), 0);
        }
        if name == "item" {
            if let Some(Some(arg)) = args.as_ref().and_then(|a| a.argnlist.first()) {
                let opt = arg.verbatim(src).trim_matches(|c| "[] ".contains(c));
                return (format!("{opt}: "), 0);
            }
            return (String::new(), 0);
        }
        if MACROS_ESCAPED.contains(&name) {
            return (name.into(), 0);
        }
        if self.custom_macro(name).is_some() {
            // The following nodes' own sources travel with them.
            let owned: Vec<LNode> = following.iter().map(|(n, _)| (*n).clone()).collect();
            let (expansion, consumed) =
                self.expand_custom_macro_invocation_mixed(node, &owned, following);
            if self.custom_macro_num_args.get(name).copied().unwrap_or(0) > 0 {
                return (self.parse_latex_fragment_to_text(&expansion), consumed);
            }
            return (expansion, consumed);
        }
        if MACROS_SPACING.contains(&name) || MACROS_IGNORED.contains(&name) {
            return (String::new(), 0);
        }
        let mut arg_parts = Vec::new();
        if let Some(a) = args {
            for arg in a.argnlist.iter().flatten() {
                let text = match arg.nodelist() {
                    Some(list) => self.nodes_to_text(list, src),
                    None => arg
                        .verbatim(src)
                        .trim_matches(|c| "{} ".contains(c))
                        .to_string(),
                };
                if !text.is_empty() {
                    arg_parts.push(text);
                }
            }
        }
        (arg_parts.join(" "), 0)
    }

    /// [`Self::expand_custom_macro_invocation`] where each following node
    /// carries its own source string.
    fn expand_custom_macro_invocation_mixed(
        &self,
        node: &LNode,
        _owned: &[LNode],
        following: &[Src],
    ) -> (String, usize) {
        let Some(name) = node.macro_name() else {
            return (String::new(), 0);
        };
        let def = self.custom_macro(name).unwrap_or("").to_string();
        let expected = self.custom_macro_num_args.get(name).copied().unwrap_or(0);
        if expected == 0 {
            return (def, 0);
        }
        let mut values: Vec<String> = Vec::new();
        let mut consumed = 0;
        for (next, src) in following {
            if values.len() >= expected {
                break;
            }
            match &next.kind {
                Kind::Chars(c) if c.trim().is_empty() => consumed += 1,
                Kind::Group { nodelist, .. } => {
                    values.push(self.nodes_to_text(nodelist, src));
                    consumed += 1;
                }
                _ => break,
            }
        }
        if values.len() < expected {
            return (def, 0);
        }
        let mut expansion = def;
        for idx in (1..=values.len()).rev() {
            expansion = expansion.replace(&format!("#{idx}"), &values[idx - 1]);
        }
        (expansion, consumed)
    }

    fn is_text_only_group(&self, node: &LNode) -> bool {
        let Some(list) = node.nodelist() else {
            return true;
        };
        for n in list {
            match &n.kind {
                Kind::Env { .. } => return false,
                Kind::Macro { name, .. } if MACROS_STRUCTURAL.contains(&name.as_str()) => {
                    return false
                }
                Kind::Group { .. } if !self.is_text_only_group(n) => return false,
                _ => {}
            }
        }
        true
    }

    // -- tables (`utils/table.py`) ---------------------------------------------------

    /// `_parse_table`: cells split at `&`, rows at `\\`, `\multicolumn` /
    /// `\multirow` arguments read from the source at the node's (document)
    /// offset — into the environment's own text, as upstream does, so the
    /// lookup usually misses and, when it lands somewhere, reads whatever
    /// braces follow.
    fn parse_table(&self, node: &LNode, src: &str) -> Option<Table> {
        let Kind::Env { nodelist, .. } = &node.kind else {
            return None;
        };
        let source_latex = node.verbatim(src);
        // Cell content comes from the environment (`src`) or from a
        // re-parsed span argument (its own string, kept alive here).
        let mut fragments: Vec<Box<str>> = Vec::new();
        let mut rows: Vec<Vec<PendingCell>> = Vec::new();
        let mut current_row: Vec<PendingCell> = Vec::new();
        // (node, index into `fragments` or usize::MAX for `src`)
        let mut current_cell: Vec<(LNode, usize)> = Vec::new();

        let finish_cell = |current_cell: &mut Vec<(LNode, usize)>,
                           current_row: &mut Vec<PendingCell>,
                           fragments: &[Box<str>],
                           col_span: usize,
                           row_span: usize| {
            let items: Vec<Src> = current_cell
                .iter()
                .map(|(n, f)| {
                    (
                        n,
                        if *f == usize::MAX {
                            src
                        } else {
                            &*fragments[*f]
                        },
                    )
                })
                .collect();
            let text = self.nodes_to_text_mixed(&items).trim().to_string();
            // Upstream writes the span into the cell's end offsets (and so
            // into the grid) but leaves its `row_span` / `col_span` fields at
            // 1; here the two agree, the one place its JSON differs.
            current_row.push(PendingCell {
                text,
                col_span,
                row_span,
                placeholder: false,
            });
            current_cell.clear();
            for _ in 1..col_span {
                current_row.push(PendingCell {
                    text: String::new(),
                    col_span: 1,
                    row_span: 1,
                    placeholder: true,
                });
            }
        };

        for n in nodelist {
            match &n.kind {
                Kind::Macro { name, .. } => match name.as_str() {
                    "\\" => {
                        if !current_cell.is_empty() {
                            finish_cell(&mut current_cell, &mut current_row, &fragments, 1, 1);
                        }
                        if !current_row.is_empty() {
                            rows.push(std::mem::take(&mut current_row));
                        }
                    }
                    "multicolumn" | "multirow" => {
                        // Upstream slices the environment's text with the
                        // node's *document* offset, a character index.
                        let char_pos = src[..n.pos].chars().count();
                        let remaining: String = source_latex.chars().skip(char_pos).collect();
                        let remaining = remaining.as_str();
                        let args = parse_brace_args(remaining);
                        if args.len() >= 3 {
                            let count = args[0].trim().parse::<usize>().unwrap_or(1).max(1);
                            let content = args[2].clone();
                            if !content.is_empty() {
                                let idx = fragments.len();
                                fragments.push(content.into_boxed_str());
                                let parsed = Walker::new(&fragments[idx]).parse();
                                current_cell.extend(parsed.into_iter().map(|p| (p, idx)));
                            }
                            if name == "multicolumn" {
                                finish_cell(
                                    &mut current_cell,
                                    &mut current_row,
                                    &fragments,
                                    count,
                                    1,
                                );
                            } else {
                                finish_cell(
                                    &mut current_cell,
                                    &mut current_row,
                                    &fragments,
                                    1,
                                    count,
                                );
                            }
                        } else {
                            current_cell.push((n.clone(), usize::MAX));
                        }
                    }
                    m if TABLE_MACROS_RULE.contains(&m) || TABLE_MACROS_IGNORE.contains(&m) => {}
                    "&" => finish_cell(&mut current_cell, &mut current_row, &fragments, 1, 1),
                    _ => current_cell.push((n.clone(), usize::MAX)),
                },
                Kind::Chars(text) => {
                    if text.contains('&') {
                        let parts: Vec<&str> = text.split('&').collect();
                        for (i, part) in parts.iter().enumerate() {
                            if !part.is_empty() {
                                current_cell.push((
                                    LNode {
                                        kind: Kind::Chars(part.to_string()),
                                        pos: 0,
                                        len: 0,
                                    },
                                    usize::MAX,
                                ));
                            }
                            if i < parts.len() - 1 {
                                finish_cell(&mut current_cell, &mut current_row, &fragments, 1, 1);
                            }
                        }
                    } else {
                        current_cell.push((n.clone(), usize::MAX));
                    }
                }
                Kind::Specials(c) if c == "&" => {
                    finish_cell(&mut current_cell, &mut current_row, &fragments, 1, 1);
                }
                _ => current_cell.push((n.clone(), usize::MAX)),
            }
        }
        if !current_cell.is_empty() {
            finish_cell(&mut current_cell, &mut current_row, &fragments, 1, 1);
        }
        if !current_row.is_empty() {
            rows.push(current_row);
        }
        if rows.is_empty() {
            return None;
        }
        let num_rows = rows.len();
        let num_cols = rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut cells: Vec<TableCell> = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            for j in 0..num_cols {
                let (text, col_span, row_span) = match row.get(j) {
                    Some(c) if c.placeholder => continue,
                    Some(c) => (c.text.clone(), c.col_span, c.row_span),
                    None => (String::new(), 1, 1),
                };
                cells.push(TableCell {
                    text,
                    bbox: None,
                    start_row: i,
                    start_col: j,
                    row_span,
                    col_span,
                    column_header: false,
                    row_header: false,
                    row_section: false,
                });
            }
        }
        // docling's `TableData.grid`: every slot a cell covers shows its
        // text, later cells overwriting earlier ones.
        let mut grid = vec![vec![String::new(); num_cols]; num_rows];
        for c in &cells {
            let rows = grid
                .iter_mut()
                .take((c.start_row + c.row_span).min(num_rows))
                .skip(c.start_row);
            for row in rows {
                let slots = row
                    .iter_mut()
                    .take((c.start_col + c.col_span).min(num_cols))
                    .skip(c.start_col);
                for slot in slots {
                    *slot = c.text.clone();
                }
            }
        }
        Some(Table {
            rows: grid,
            location: None,
            structure: None,
            cell_blocks: None,
            cells: Some(cells),
            caption: None,
            caption_parent: Default::default(),
            caption_location: None,
        })
    }
}

struct PendingCell {
    text: String,
    col_span: usize,
    row_span: usize,
    placeholder: bool,
}

/// Upstream's `parse_brace_args`: every top-level `{…}` in `text`.
fn parse_brace_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' {
            let mut depth = 1;
            let start = i + 1;
            i += 1;
            while i < chars.len() && depth > 0 {
                match chars[i] {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
            let end = if depth == 0 { i - 1 } else { i };
            args.push(chars[start..end.max(start)].iter().collect());
        } else {
            i += 1;
        }
    }
    args
}

// ---------------------------------------------------------------------------
// Helpers

fn preprocess_custom_macros(text: &str) -> String {
    let pairs = [
        (r"\\be\b", "\\begin{equation}"),
        (r"\\ee\b", "\\end{equation}"),
        (r"\\bea\b", "\\begin{eqnarray}"),
        (r"\\eea\b", "\\end{eqnarray}"),
        (r"\\beq\b", "\\begin{equation}"),
        (r"\\eeq\b", "\\end{equation}"),
    ];
    let mut s = text.to_string();
    for (pat, rep) in pairs {
        if let Ok(re) = Regex::new(pat) {
            s = re.replace_all(&s, regex::NoExpand(rep)).into_owned();
        }
    }
    s
}

/// Every `\name` not followed by an ASCII letter, replaced by `def`.
fn replace_macro_calls(s: &str, name: &str, def: &str) -> String {
    let pattern = format!("\\{name}");
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(&pattern) {
        let after = &rest[i + pattern.len()..];
        out.push_str(&rest[..i]);
        if after.starts_with(|c: char| c.is_ascii_alphabetic()) {
            out.push_str(&pattern);
        } else {
            out.push_str(def);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

fn find_document_env(nodes: &[LNode], depth: usize) -> Option<&LNode> {
    if depth > 10 {
        return None;
    }
    for node in nodes {
        if node.env_name() == Some("document") {
            return Some(node);
        }
        if let Some(list) = node.nodelist().filter(|l| !l.is_empty()) {
            if let Some(found) = find_document_env(list, depth + 1) {
                return Some(found);
            }
        }
        if let Some(args) = node.args() {
            for arg in args.argnlist.iter().flatten() {
                if let Some(list) = arg.nodelist().filter(|l| !l.is_empty()) {
                    if let Some(found) = find_document_env(list, depth + 1) {
                        return Some(found);
                    }
                }
            }
        }
    }
    None
}

fn parse_custom_macro_num_args(arg: Option<&LNode>, src: &str) -> usize {
    arg.and_then(|a| {
        a.verbatim(src)
            .trim_matches(|c| "{}[] \n\t".contains(c))
            .parse::<i64>()
            .ok()
    })
    .filter(|n| *n > 0)
    .map_or(0, |n| n as usize)
}

fn heading_level(name: &str) -> u8 {
    match name {
        "part" | "chapter" | "section" => 1,
        "subsection" => 2,
        "subsubsection" => 3,
        "paragraph" => 4,
        "subparagraph" => 5,
        _ => 1,
    }
}

/// Python's `str.capitalize()`.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

/// `_extract_verbatim_content`: the body between `\begin{env}[…]` and
/// `\end{env}`, stripped; the whole text when the pattern fails.
fn extract_verbatim_content(latex: &str, env: &str) -> String {
    let begin = format!("\\begin{{{env}}}");
    let end = format!("\\end{{{env}}}");
    let Some(start) = latex.find(&begin) else {
        return latex.to_string();
    };
    let mut body_start = start + begin.len();
    // An optional `[…]` right after `\begin{env}` (non-greedy, may span lines).
    if latex[body_start..].starts_with('[') {
        if let Some(close) = latex[body_start..].find(']') {
            body_start += close + 1;
        }
    }
    match latex[body_start..].find(&end) {
        Some(stop) => latex[body_start..body_start + stop].trim().to_string(),
        None => latex.to_string(),
    }
}

fn validate_tikz_nodelist(nodes: &[LNode], src: &str, depth: usize) -> bool {
    if depth > 50 {
        return false;
    }
    for node in nodes {
        if node.env_name() == Some("tikzpicture") && !tikz_end_re().is_match(node.verbatim(src)) {
            return false;
        }
        if let Some(list) = node.nodelist() {
            if !validate_tikz_nodelist(list, src, depth + 1) {
                return false;
            }
        }
        if let Some(args) = node.args() {
            for arg in args.argnlist.iter().flatten() {
                if let Some(list) = arg.nodelist() {
                    if !validate_tikz_nodelist(list, src, depth + 1) {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// `path` resolved, when it stays under `base` (upstream's traversal guard).
fn within(base: &Path, path: &Path) -> Option<PathBuf> {
    let base = base.canonicalize().ok()?;
    let resolved = path.canonicalize().ok()?;
    resolved.starts_with(&base).then_some(resolved)
}

fn tikz_end_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\\end\s*\{\s*tikzpicture\s*\}").expect("tikz regex"))
}

fn label_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\\label\{[^\n]*?\}").expect("label regex"))
}

fn spaces_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(" +").expect("spaces regex"))
}

fn blank_lines_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new("\n\n+").expect("blank lines regex"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::InputFormat;

    fn convert(tex: &str) -> DoclingDocument {
        let src = SourceDocument::from_bytes("d", InputFormat::Latex, tex.as_bytes().to_vec());
        LatexBackend.convert(&src).unwrap()
    }

    #[test]
    fn paragraph_breaks_and_inline_macros_follow_docling() {
        let tex = "\\begin{document}\nA \\textit{it} text. B \\textbf{bf} more.\n\n\
            C \\emph{em}x and \\textit{it}.\n\\end{document}";
        let md = convert(tex).export_to_markdown();
        // The space after a formatting macro's argument is ordinary text (only
        // the space after a macro *name* is swallowed); a sentence continuing
        // after a paragraph break stays one paragraph (docling#4340).
        assert_eq!(md.trim(), "A it text. B bf more.\n\nC emx and it.");
    }

    #[test]
    fn tabular_keeps_the_trailing_blank_row() {
        let tex = "\\begin{document}\n\\begin{tabular}{|c|c|}\n\\hline\nH1 & H2 \\\\\n\\hline\n\
            a & b \\\\\n\\hline\n\\end{tabular}\n\\end{document}";
        let md = convert(tex).export_to_markdown();
        assert!(
            md.contains("| H1   | H2   |\n|------|------|\n| a    | b    |\n|      |      |"),
            "got:\n{md}"
        );
    }

    #[test]
    fn tree_labels_and_list_meta_follow_docling() {
        let tex = "\\title{T}\\author{A}\n\\begin{document}\n\\maketitle\n\n            \\section{Math}\n\nInline math: $x$\n\nDisplay:\n$$y = 1$$\n\n            \\subsection{List}\nAfter a heading \\textbf{bold} text.\n\n            \\begin{enumerate}\n\\item one\n\\item two\n\\end{enumerate}\n\n            \\begin{tabular}{cc}\n\\hline\na & b \\\\\n\\hline\n\\end{tabular}\n\\end{document}";
        let json: serde_json::Value = serde_json::from_str(&convert(tex).export_to_json()).unwrap();
        let texts = json["texts"].as_array().unwrap();
        let labels: Vec<(&str, &str)> = texts
            .iter()
            .map(|t| (t["label"].as_str().unwrap(), t["text"].as_str().unwrap()))
            .collect();
        assert_eq!(
            labels,
            [
                ("title", "T"),
                ("text", "A"),
                ("section_header", "Math"),
                ("text", "Inline math: $x$"),
                ("text", "Display:"),
                ("formula", "y = 1"),
                ("section_header", "List"),
                ("text", "After a heading bold text."),
                ("list_item", "one"),
                ("list_item", "two"),
            ]
        );
        assert_eq!(texts[2]["level"], 1);
        assert_eq!(texts[6]["level"], 2);
        assert_eq!(texts[9]["enumerated"], false);
        assert_eq!(texts[9]["marker"], "");
        assert_eq!(texts[9]["parent"]["$ref"], "#/groups/0");
        assert_eq!(json["groups"][0]["label"], "list");
        let data = &json["tables"][0]["data"];
        assert_eq!(data["num_rows"], 2);
        assert_eq!(data["num_cols"], 2);
        assert_eq!(data["table_cells"].as_array().unwrap().len(), 4);
    }

    /// docling#4325: `tabular*`, `tabularx` and `longtable` are tables too;
    /// longtable's column specification is an argument (docling's own spec),
    /// its `\\endhead` marker no cell content.
    #[test]
    fn wide_and_long_tables_are_tables() {
        let tex = "\\begin{document}\n\\begin{longtable}{cc}\nH1 & H2 \\\\ \\endhead\na & b \\\\\n\\end{longtable}\n\
            \\begin{tabularx}{\\textwidth}{XX}\nc & d \\\\\n\\end{tabularx}\n\\end{document}";
        let json: serde_json::Value = serde_json::from_str(&convert(tex).export_to_json()).unwrap();
        let tables = json["tables"].as_array().unwrap();
        assert_eq!(tables.len(), 2, "{json}");
        let cells = |t: usize| -> Vec<String> {
            tables[t]["data"]["table_cells"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["text"].as_str().unwrap().to_string())
                .collect()
        };
        // Each ends with docling's trailing blank row (the newline after
        // the last `\\` is a chars node that finishes one more row).
        assert_eq!(cells(0), ["H1", "H2", "a", "b", "", ""]);
        assert_eq!(cells(1), ["c", "d", "", ""]);
    }

    #[test]
    fn figures_captions_theorems_and_custom_macros() {
        let tex = "\\newcommand{\\ours}{OTSL}\\newcommand{\\emp}[1]{<#1>}\n\\begin{document}\n\
            \\begin{figure}\\centering\\includegraphics[width=1cm]{fig/a_b.png}\\caption{A fig.}\\label{f}\\end{figure}\n\
            \\begin{theorem}Text with \\ours{} and \\emp{x}.\\end{theorem}\n\
            Cite~\\cite{a,b}; dashes -- gone.\n\\end{document}";
        let doc = convert(tex);
        let md = doc.export_to_markdown();
        assert_eq!(
            md.trim(),
            "Image: fig/a\\_b.png\n\n<!-- image -->\n\nA fig.\n\n**Theorem.**\n\nText with OTSL and &lt;x&gt;.\n\nCite[a,b]; dashes  gone."
        );
        let json: serde_json::Value = serde_json::from_str(&doc.export_to_json()).unwrap();
        // The `Image:` caption is a body text the picture references; the
        // picture and `\caption` text are children of the figure group.
        assert_eq!(json["texts"][0]["parent"]["$ref"], "#/body");
        assert_eq!(json["pictures"][0]["captions"][0]["$ref"], "#/texts/0");
        assert_eq!(json["pictures"][0]["parent"]["$ref"], "#/groups/0");
        assert_eq!(json["groups"][0]["name"], "figure");
        // `\caption` is unknown to pylatexenc: its argument is a sibling
        // group, flushed as a `text` item by the `\label` that follows.
        assert_eq!(json["texts"][1]["label"], "text");
        assert_eq!(json["texts"][1]["parent"]["$ref"], "#/groups/0");
    }
}
