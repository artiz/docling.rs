//! LaTeX serializer — the Rust counterpart of docling-core's
//! `LaTeXDocSerializer` with default `LaTeXParams` (docling 2.124's
//! `--to latex`, #317), byte-for-byte on the declarative corpus.
//!
//! The Python serializer walks the *JSON* document model, so this one mirrors
//! what [`crate::json`] emits for each [`Node`] rather than the node itself:
//! a level-1 heading is a `title` item, deeper headings are `section_header`s
//! one level down, a `$$…$$` paragraph is a formula item, runs of list items
//! fold into (sibling / nested) list groups, form regions are `field_region`
//! / `field_item` items the upstream fallback renders as `% missing-text`,
//! and everything docling keeps out of the JSON body (furniture, page breaks,
//! DocLang-only nodes) is absent here too. Text goes through the same
//! `unescape_text` the JSON export applies, then LaTeX escaping — exactly the
//! `post_process` order upstream uses.
//!
//! Upstream quirks reproduced on purpose: the first `\title{…}` without nested
//! braces is hoisted into the preamble (a `\maketitle` takes its place), the
//! table grid repeats a spanning cell's text over every covered position, and
//! the placeholder picture is `% image` whether or not an image is attached.
//! One deliberate deviation: upstream *raises* on a section header deeper
//! than `\subsubsection`; a conversion should not fail on an `<h5>`, so those
//! degrade to `\paragraph` / `\subparagraph`.

use crate::document::{DoclingDocument, FieldItem, Node, Table};

const DOCUMENT_CLASS: &str = r"\documentclass[11pt,a4paper]{article}";
const PACKAGES: [&str; 11] = [
    r"\usepackage[utf8]{inputenc} % allow utf-8 input",
    r"\usepackage[T1]{fontenc}    % use 8-bit T1 fonts",
    r"\usepackage{hyperref}       % hyperlinks",
    r"\usepackage{url}            % simple URL typesetting",
    r"\usepackage{booktabs}       % professional-quality tables",
    r"\usepackage{amsfonts}       % blackboard math symbols",
    r"\usepackage{nicefrac}       % compact symbols for 1/2, etc.",
    r"\usepackage{microtype}      % microtypography",
    r"\usepackage{xcolor}         % colors",
    r"\usepackage{graphicx}       % graphics",
    r"\usepackage[normalem]{ulem} % strikethrough",
];
const IMAGE_PLACEHOLDER: &str = "% image";
const MISSING_TEXT: &str = "% missing-text";
/// Spaces per nesting level on a nested list's `\begin`/`\end` lines.
const INDENT: usize = 2;

/// Serialize `doc` to a complete LaTeX document. No trailing newline — the
/// upstream CLI writes the serializer's text verbatim into `<stem>.tex`.
pub fn to_latex(doc: &DoclingDocument) -> String {
    let parts = render_nodes(&doc.nodes, 1, false);
    let mut body = parts.join("\n\n");

    // Hoist the first `\title{…}` (no nested braces — upstream's regex) into
    // the preamble, drop every such occurrence from the body, collapse the
    // blank runs that leaves behind.
    let mut title_cmd: Option<String> = None;
    if let Some(first) = find_title(&body, 0) {
        title_cmd = Some(format!("\\title{{{}}}", &body[first.1..first.2]));
        let mut out = String::with_capacity(body.len());
        let mut pos = 0;
        let mut cur = Some(first);
        while let Some((start, _, end)) = cur {
            out.push_str(&body[pos..start]);
            pos = end + 1;
            cur = find_title(&body, pos);
        }
        out.push_str(&body[pos..]);
        body = collapse_blank_runs(&out).trim().to_string();
    }

    let mut preamble: Vec<&str> = vec![DOCUMENT_CLASS, ""];
    preamble.extend(PACKAGES);
    let mut header = preamble.join("\n");
    if let Some(t) = &title_cmd {
        header.push('\n');
        header.push_str(t);
    }
    header.push_str("\n\n\\begin{document}");

    let mut blocks: Vec<&str> = Vec::new();
    if title_cmd.is_some() {
        blocks.push("\\maketitle");
    }
    if !body.is_empty() {
        blocks.push(&body);
    }
    if blocks.is_empty() {
        format!("{header}\n\n\\end{{document}}")
    } else {
        format!("{header}\n\n{}\n\n\\end{{document}}", blocks.join("\n\n"))
    }
}

/// Upstream's `\\title\s*\{([^{}]*)\}`: returns (match start, content start,
/// closing-brace index) of the first match at or after `from`.
fn find_title(text: &str, from: usize) -> Option<(usize, usize, usize)> {
    let mut search = from;
    while let Some(rel) = text[search..].find("\\title") {
        let start = search + rel;
        let mut i = start + "\\title".len();
        let bytes = text.as_bytes();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes.get(i) == Some(&b'{') {
            let content_start = i + 1;
            if let Some(rel_end) = text[content_start..].find(['{', '}']) {
                let end = content_start + rel_end;
                if bytes[end] == b'}' {
                    return Some((start, content_start, end));
                }
            }
        }
        search = start + 1;
    }
    None
}

/// `re.sub(r"\n{3,}", "\n\n", …)`.
fn collapse_blank_runs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut newlines = 0;
    for ch in text.chars() {
        if ch == '\n' {
            newlines += 1;
            if newlines <= 2 {
                out.push(ch);
            }
        } else {
            newlines = 0;
            out.push(ch);
        }
    }
    out
}

/// Upstream's `_escape_latex`: single pass, character by character.
pub fn escape_latex(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str(r"\textbackslash{}"),
            '{' => out.push_str(r"\{"),
            '}' => out.push_str(r"\}"),
            '#' => out.push_str(r"\#"),
            '$' => out.push_str(r"\$"),
            '%' => out.push_str(r"\%"),
            '&' => out.push_str(r"\&"),
            '_' => out.push_str(r"\_"),
            '~' => out.push_str(r"\textasciitilde{}"),
            '^' => out.push_str(r"\textasciicircum{}"),
            c => out.push(c),
        }
    }
    out
}

/// The JSON export's `unescape_text`: the model text is the Markdown-free
/// raw string, which is what upstream then LaTeX-escapes.
fn unescape_text(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("\\_", "_")
}

fn text_item(s: &str) -> String {
    escape_latex(&unescape_text(s))
}

/// Serialize a sibling run of nodes into the non-empty parts upstream's
/// `get_parts` would collect at this level. `list_level` is the nesting depth
/// a list group *started here* renders at (1 at the document body: the outer
/// `\begin{itemize}` is unindented, its nested groups indent by one step).
fn render_nodes(nodes: &[Node], list_level: usize, inline: bool) -> Vec<String> {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < nodes.len() {
        if matches!(nodes[i], Node::ListItem { .. }) {
            // Fold the run exactly like the JSON export's `walk_into`: list
            // items plus an empty paragraph sitting between two of them.
            let start = i;
            i += 1;
            loop {
                match nodes.get(i) {
                    Some(Node::ListItem { .. }) => i += 1,
                    Some(Node::Paragraph { text })
                        if text.is_empty()
                            && matches!(nodes.get(i + 1), Some(Node::ListItem { .. })) =>
                    {
                        i += 1
                    }
                    _ => break,
                }
            }
            for group in sibling_lists(&nodes[start..i]) {
                let text = render_list(group, list_level - 1);
                if !text.is_empty() {
                    parts.push(text);
                }
            }
        } else {
            render_one(&nodes[i], list_level, inline, &mut parts);
            i += 1;
        }
    }
    parts
}

fn render_one(node: &Node, list_level: usize, inline: bool, parts: &mut Vec<String>) {
    let push = |parts: &mut Vec<String>, s: String| {
        if !s.is_empty() {
            parts.push(s);
        }
    };
    match node {
        // JSON: level 1 is the `title` item; deeper levels are section headers
        // one level down (1 → \section … 3 → \subsubsection).
        Node::Heading { level: 1, text } => push(parts, format!("\\title{{{}}}", text_item(text))),
        Node::Heading { level, text } => {
            let cmd = match level {
                2 => "section",
                3 => "subsection",
                4 => "subsubsection",
                // Upstream raises here; degrade instead (module docs).
                5 => "paragraph",
                _ => "subparagraph",
            };
            push(parts, format!("\\{cmd}{{{}}}", text_item(text)));
        }
        Node::Paragraph { text } => {
            // A whole-paragraph `$$…$$` is a formula item in the JSON — never
            // escaped, re-wrapped by the serializer.
            let t = text.trim();
            match t.strip_prefix("$$").and_then(|s| s.strip_suffix("$$")) {
                Some(inner) if !inner.is_empty() => push(parts, formula(inner, inline)),
                _ => push(parts, text_item(text)),
            }
        }
        Node::CheckboxItem { checked, text } => {
            let mark = if *checked { "- [x] " } else { "- [ ] " };
            push(parts, text_item(&format!("{mark}{text}")));
        }
        Node::Code { text, .. } => {
            let raw = unescape_text(text);
            if inline {
                // Upstream escapes only `#` in inline code — and does so with a
                // doubled backslash (its `r"\\#"`), reproduced verbatim.
                push(parts, format!("\\texttt{{{}}}", raw.replace('#', "\\\\#")));
            } else {
                push(
                    parts,
                    format!("\\begin{{verbatim}}\n{raw}\n\\end{{verbatim}}"),
                );
            }
        }
        Node::Formula { latex, orig, .. } => {
            if !latex.is_empty() {
                push(parts, formula(latex, inline));
            } else if !orig.is_empty() {
                push(parts, "% formula-not-decoded".to_string());
            }
        }
        Node::Table(t) => push(parts, render_table(t)),
        Node::Picture {
            caption,
            classification,
            ..
        } => {
            let class = classification
                .as_deref()
                .and_then(|c| c.first())
                .map(|c| c.class_name.replace('_', " "));
            push(parts, render_figure(caption.as_deref(), class.as_deref()));
        }
        // A chart is a picture item without image payload in the JSON.
        Node::Chart { caption, .. } => push(parts, render_figure(caption.as_deref(), None)),
        // Upstream's fallback on a group joins its children with blank lines;
        // an `inline` group is docling's InlineGroup, joined with spaces in
        // inline scope.
        Node::Group { label, children } => {
            if label == "inline" {
                push(parts, render_nodes(children, list_level, true).join(" "));
            } else {
                push(
                    parts,
                    render_nodes(children, list_level, inline).join("\n\n"),
                );
            }
        }
        Node::FieldRegion { items } => render_field_region(items, parts),
        Node::InlineGroup { md_text, .. } => push(parts, text_item(md_text)),
        Node::TextDump(text) => push(parts, text_item(text)),
        Node::Located { inner, .. } => render_one(inner, list_level, inline, parts),
        // A lone list item outside a run (defensive; `render_nodes` folds
        // every run it sees).
        Node::ListItem { .. } => {
            let text = render_list(std::slice::from_ref(node), list_level - 1);
            push(parts, text);
        }
        // Not in the JSON body, so not in upstream's output.
        Node::DoclangOnly(_)
        | Node::Furniture { .. }
        | Node::PageFurniture { .. }
        | Node::PageBreak
        | Node::PageInfo { .. } => {}
    }
}

fn formula(latex: &str, inline: bool) -> String {
    if inline {
        format!("${latex}$")
    } else {
        format!("$${latex}$$")
    }
}

/// `field_region` / `field_item` items are neither text nor groups to the
/// upstream serializer: each falls back to `% missing-text`, and the item's
/// marker / key / value children follow as plain text items (the outer walk
/// still descends into them).
fn render_field_region(items: &[FieldItem], parts: &mut Vec<String>) {
    parts.push(MISSING_TEXT.to_string());
    for item in items {
        parts.push(MISSING_TEXT.to_string());
        for text in [&item.marker, &item.key, &item.value].into_iter().flatten() {
            let t = text_item(text);
            if !t.is_empty() {
                parts.push(t);
            }
        }
    }
}

fn render_figure(caption: Option<&str>, class: Option<&str>) -> String {
    let mut lines = vec![
        "\\begin{figure}[h]".to_string(),
        IMAGE_PLACEHOLDER.to_string(),
    ];
    if let Some(cap) = caption.filter(|c| !c.is_empty()) {
        lines.push(format!("\\caption{{{}}}", text_item(cap)));
    }
    if let Some(class) = class {
        lines.push(format!("% annotation[classification]: {class}"));
    }
    lines.push("\\end{figure}".to_string());
    lines.join("\n")
}

/// The `table` environment over a `|l|…|l|` tabular of the JSON grid: with
/// first-class cells a spanning cell's text repeats over every position it
/// covers (holes are empty); otherwise the padded row grid.
fn render_table(t: &Table) -> String {
    let num_rows = t.rows.len();
    let num_cols = t.rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut grid: Vec<Vec<String>> = Vec::with_capacity(num_rows);
    match t.cells.as_ref().filter(|c| !c.is_empty()) {
        Some(cells) => {
            let mut by_pos: std::collections::HashMap<(usize, usize), &str> =
                std::collections::HashMap::new();
            for c in cells {
                for r in c.start_row..(c.start_row + c.row_span).min(num_rows) {
                    for k in c.start_col..(c.start_col + c.col_span).min(num_cols) {
                        by_pos.insert((r, k), c.text.as_str());
                    }
                }
            }
            for r in 0..num_rows {
                grid.push(
                    (0..num_cols)
                        .map(|c| cell_text(by_pos.get(&(r, c)).copied().unwrap_or("")))
                        .collect(),
                );
            }
        }
        None => {
            for row in &t.rows {
                grid.push(
                    (0..num_cols)
                        .map(|c| cell_text(row.get(c).map(String::as_str).unwrap_or("")))
                        .collect(),
                );
            }
        }
    }
    let mut tabular = String::new();
    if !grid.is_empty() {
        let colspec = format!("|{}|", vec!["l"; num_cols].join("|"));
        let mut lines = vec![
            format!("\\begin{{tabular}}{{{colspec}}}"),
            "\\hline".to_string(),
        ];
        for row in &grid {
            lines.push(format!("{} \\\\ \\hline", row.join(" & ")));
        }
        lines.push("\\end{tabular}".to_string());
        tabular = lines.join("\n");
    }
    let caption = t
        .caption
        .as_deref()
        .filter(|c| !c.is_empty())
        .map(text_item)
        .unwrap_or_default();
    if tabular.is_empty() && caption.is_empty() {
        return String::new();
    }
    let mut content = vec!["\\begin{table}[h]".to_string()];
    if !caption.is_empty() {
        content.push(format!("\\caption{{{caption}}}"));
    }
    if !tabular.is_empty() {
        content.push(tabular);
    }
    content.push("\\end{table}".to_string());
    content.join("\n")
}

fn cell_text(s: &str) -> String {
    text_item(s).replace('\n', " ")
}

// ---------------------------------------------------------------------------
// Lists — the grouping mirrors `json.rs` (`add_sibling_lists` / `add_list`).
// ---------------------------------------------------------------------------

fn level_of(node: &Node) -> u8 {
    match node {
        Node::ListItem { level, .. } => *level,
        _ => 0,
    }
}

/// Split a run of list items into sibling list groups at the base level: a
/// new group starts on `first_in_list`, a kind flip, or an ordered-number
/// discontinuity — unless the previous base item was a multilevel projection
/// continuing the same Word list (docling#3902).
fn sibling_lists(run: &[Node]) -> Vec<&[Node]> {
    let base = level_of(&run[0]);
    let mut groups = Vec::new();
    let mut seg = 0;
    let mut prev: Option<(bool, u64)> = None;
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
            continue;
        }
        let eff_ordered = dclx.as_ref().map_or(*ordered, |d| d.ordered);
        if k > seg {
            if let Some((po, pn)) = prev {
                let same_word_list = prev_projected && eff_ordered;
                if *first_in_list
                    || (!same_word_list && (po != *ordered || (*ordered && *number != pn + 1)))
                {
                    groups.push(&run[seg..k]);
                    seg = k;
                }
            }
        }
        prev = Some((*ordered, *number));
        prev_projected = eff_ordered && !*ordered;
    }
    groups.push(&run[seg..]);
    groups
}

/// One list group: `\begin{env}` … `\end{env}`, both indented by `depth`
/// steps; items are never indented. An item's deeper successors nest under
/// it as further groups (each their own environment, one step deeper).
fn render_list(items: &[Node], depth: usize) -> String {
    let base = level_of(&items[0]);
    let enumerated = items.iter().find_map(|n| match n {
        Node::ListItem { level, ordered, .. } if *level == base => Some(*ordered),
        _ => None,
    });
    let env = if enumerated.unwrap_or(false) {
        "enumerate"
    } else {
        "itemize"
    };
    let mut lines: Vec<String> = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let Node::ListItem { text, .. } = &items[i] else {
            i += 1;
            continue;
        };
        if level_of(&items[i]) > base {
            i += 1;
            continue;
        }
        lines.push(format!("\\item {}", text_item(text)));
        let mut j = i + 1;
        while j < items.len() && level_of(&items[j]) > base {
            j += 1;
        }
        if j > i + 1 {
            let nested: Vec<&Node> = items[i + 1..j]
                .iter()
                .filter(|n| matches!(n, Node::ListItem { .. }))
                .collect();
            if !nested.is_empty() {
                let owned: Vec<Node> = nested.into_iter().cloned().collect();
                for group in sibling_lists(&owned) {
                    let text = render_list(group, depth + 1);
                    if !text.is_empty() {
                        lines.push(text);
                    }
                }
            }
        }
        i = j;
    }
    if lines.is_empty() {
        return String::new();
    }
    let indent = " ".repeat(depth * INDENT);
    format!(
        "{indent}\\begin{{{env}}}\n{}\n{indent}\\end{{{env}}}",
        lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_every_special_character_once() {
        assert_eq!(
            escape_latex(r"a\b{c}#$%&_~^"),
            r"a\textbackslash{}b\{c\}\#\$\%\&\_\textasciitilde{}\textasciicircum{}"
        );
    }

    #[test]
    fn title_is_hoisted_and_maketitle_added() {
        let mut doc = DoclingDocument::new("t");
        doc.push(Node::Heading {
            level: 1,
            text: "Hello 100%".into(),
        });
        doc.push(Node::Paragraph {
            text: "Body.".into(),
        });
        let tex = to_latex(&doc);
        assert!(tex.contains("\\title{Hello 100\\%}\n\n\\begin{document}\n\n\\maketitle\n\nBody."));
        assert!(!tex.ends_with('\n'));
    }

    #[test]
    fn title_with_braces_stays_in_the_body() {
        // Upstream's hoisting regex refuses nested braces: an escaped `{`
        // in the title keeps the `\title` inline and adds no `\maketitle`.
        let mut doc = DoclingDocument::new("t");
        doc.push(Node::Heading {
            level: 1,
            text: "a {b}".into(),
        });
        let tex = to_latex(&doc);
        assert!(tex.contains("\\begin{document}\n\n\\title{a \\{b\\}}\n\n\\end{document}"));
        assert!(!tex.contains("\\maketitle"));
    }

    #[test]
    fn nested_lists_indent_their_environments_only() {
        let item = |level: u8, text: &str, first: bool| Node::ListItem {
            ordered: false,
            number: 1,
            first_in_list: first,
            text: text.into(),
            level,
            marker: None,
            location: None,
            dclx: None,
            href: None,
            layer: None,
        };
        let mut doc = DoclingDocument::new("t");
        doc.push(item(0, "a", true));
        doc.push(item(1, "a1", true));
        doc.push(item(0, "b", false));
        let tex = to_latex(&doc);
        assert!(
            tex.contains(
                "\\begin{itemize}\n\\item a\n  \\begin{itemize}\n\\item a1\n  \\end{itemize}\n\\item b\n\\end{itemize}"
            ),
            "{tex}"
        );
    }

    #[test]
    fn empty_document_is_just_the_skeleton() {
        let tex = to_latex(&DoclingDocument::new("t"));
        assert!(tex.ends_with("\\begin{document}\n\n\\end{document}"));
    }
}
