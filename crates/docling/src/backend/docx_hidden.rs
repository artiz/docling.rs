//! Hidden text (`w:vanish`, Word's Hidden font property) — docling#4958.
//!
//! Word neither shows nor prints hidden text, and docling 2.143 leaves it out
//! of the default export: a paragraph whose every text run is hidden goes to
//! the `invisible` content layer (kept, like a hidden slide or sheet, but out
//! of the Markdown), and a hidden run inside a shown paragraph — which has no
//! layer of its own — is dropped from the text. A run is hidden by its own
//! `w:rPr/w:vanish`, else its character style's `basedOn` chain, else the
//! paragraph style's chain; the first explicit setting wins.
//!
//! [`resolve_hidden`] settles this on the XML before it is parsed, as
//! `mc.rs` settles `mc:AlternateContent`, so both DOCX walks read the same
//! document: a fully hidden paragraph is marked with [`HIDDEN_ATTR`] (each
//! walk puts what it yields on the `invisible` layer) and a hidden text run
//! of a shown paragraph is cut out. A fully hidden paragraph in a table cell
//! or a note is cut out whole — neither has a layer per paragraph, which is
//! how docling leaves them out too. A run carrying a drawing, picture,
//! embedded object or equation is never cut: only text goes.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use roxmltree::{Document, Node};

use super::docx::on_off;

/// The attribute marking a fully hidden paragraph (no namespace: it cannot
/// collide with OOXML's).
pub(super) const HIDDEN_ATTR: &str = "docling-hidden";
/// [`HIDDEN_ATTR`] as inserted after a paragraph's tag name.
const MARK: &str = " docling-hidden=\"1\"";

/// Whether `p` was marked fully hidden by [`resolve_hidden`].
pub(super) fn is_hidden_paragraph(p: Node) -> bool {
    p.attribute(HIDDEN_ATTR).is_some()
}

/// What the hidden-text rules read from `styles.xml`.
#[derive(Default)]
pub(super) struct HiddenStyles {
    /// styleId → the first `w:rPr/w:vanish` on its `basedOn` chain.
    chain: HashMap<String, Option<bool>>,
    /// Character styles (what `w:rStyle` may name).
    character: HashSet<String>,
    /// Paragraph styles (what `w:pStyle` may name).
    paragraph: HashSet<String>,
    default_paragraph: Option<String>,
    default_character: Option<String>,
    /// `styles.xml` carries a `w:vanish` at all.
    any: bool,
}

impl HiddenStyles {
    /// Read the styles part (`styles_xml`); `max_depth` bounds a `basedOn`
    /// chain (docling's `_MAX_STYLE_INHERITANCE_DEPTH`).
    pub(super) fn parse(styles_xml: &str, max_depth: usize) -> Self {
        let mut out = HiddenStyles {
            any: styles_xml.contains("vanish"),
            ..Default::default()
        };
        let Ok(dom) = Document::parse(styles_xml) else {
            return out;
        };
        let mut own: HashMap<String, Option<bool>> = HashMap::new();
        let mut based: HashMap<String, String> = HashMap::new();
        for style in dom.descendants().filter(|n| n.has_tag_name("style")) {
            let Some(id) = local_attr(style, "styleId") else {
                continue;
            };
            let kind = local_attr(style, "type").unwrap_or("paragraph");
            let default = local_attr(style, "default").is_some_and(|v| on_off(Some(v)));
            match kind {
                "paragraph" => {
                    out.paragraph.insert(id.to_string());
                    if default {
                        out.default_paragraph = Some(id.to_string());
                    }
                }
                "character" => {
                    out.character.insert(id.to_string());
                    if default {
                        out.default_character = Some(id.to_string());
                    }
                }
                _ => {}
            }
            let vanish = child(style, "rPr")
                .and_then(|pr| child(pr, "vanish"))
                .map(|v| on_off(local_attr(v, "val")));
            own.insert(id.to_string(), vanish);
            if let Some(b) = child(style, "basedOn").and_then(|b| local_attr(b, "val")) {
                based.insert(id.to_string(), b.to_string());
            }
        }
        for id in own.keys() {
            let mut cur = id.as_str();
            let mut found = None;
            for _ in 0..max_depth {
                if let Some(v) = own.get(cur).copied().flatten() {
                    found = Some(v);
                    break;
                }
                match based.get(cur) {
                    Some(next) if own.contains_key(next) => cur = next,
                    _ => break,
                }
            }
            out.chain.insert(id.clone(), found);
        }
        out
    }

    /// The paragraph style python-docx's `paragraph.style` resolves `p` to:
    /// its `w:pStyle` when that names a paragraph style, else the default.
    fn paragraph_style<'a>(&'a self, p: Node<'a, '_>) -> Option<&'a str> {
        let named = child(p, "pPr")
            .and_then(|pr| child(pr, "pStyle"))
            .and_then(|s| local_attr(s, "val"))
            .filter(|id| self.paragraph.contains(*id));
        named.or(self.default_paragraph.as_deref())
    }

    /// `_is_hidden_run`: the run's own setting, then its character style
    /// chain (`w:rStyle`, or the default character style), then the
    /// paragraph style's chain.
    fn run_hidden(&self, run: Node, paragraph_style: Option<&str>) -> bool {
        let rpr = child(run, "rPr");
        if let Some(v) = rpr.and_then(|pr| child(pr, "vanish")) {
            return on_off(local_attr(v, "val"));
        }
        let character = rpr
            .and_then(|pr| child(pr, "rStyle"))
            .and_then(|s| local_attr(s, "val"))
            .filter(|id| self.character.contains(*id))
            .or(self.default_character.as_deref());
        for style in [character, paragraph_style].into_iter().flatten() {
            if let Some(v) = self.chain.get(style).copied().flatten() {
                return v;
            }
        }
        false
    }
}

/// `xml` (a document, header, footer or notes part) with its hidden text
/// settled — see the module docs. `notes`: the part is footnotes/endnotes,
/// where a fully hidden paragraph is cut out rather than marked. A part
/// without any `w:vanish` (nor styles with one) is returned as is.
pub(super) fn resolve_hidden<'x>(xml: &'x str, styles: &HiddenStyles, notes: bool) -> Cow<'x, str> {
    if !styles.any && !xml.contains("vanish") {
        return Cow::Borrowed(xml);
    }
    let Ok(doc) = Document::parse(xml) else {
        return Cow::Borrowed(xml);
    };
    // (range, replacement): a mark is an insertion (empty range), a cut an
    // empty replacement. Ranges never overlap: nothing inside a fully hidden
    // paragraph is visited, and a cut run holds no paragraph.
    let mut edits: Vec<(std::ops::Range<usize>, &str)> = Vec::new();
    let mut skip_inside: Option<std::ops::Range<usize>> = None;
    for p in doc
        .descendants()
        .filter(|n| n.has_tag_name("p") && is_w(*n))
    {
        if skip_inside
            .as_ref()
            .is_some_and(|r| r.contains(&p.range().start))
        {
            continue;
        }
        let pstyle = styles.paragraph_style(p);
        // `_is_hidden_paragraph`: text runs anywhere in it, all hidden, and
        // no equation (its runs are math runs: always shown).
        let has_math = p
            .descendants()
            .any(|n| n.tag_name().name() == "oMath" && !is_w(n));
        let text_runs: Vec<Node> = p
            .descendants()
            .filter(|n| n.has_tag_name("r") && is_w(*n) && child(*n, "t").is_some())
            .collect();
        let fully_hidden = !has_math
            && !text_runs.is_empty()
            && text_runs.iter().all(|r| styles.run_hidden(*r, pstyle));
        if fully_hidden {
            let in_cell = p
                .ancestors()
                .skip(1)
                .any(|a| a.has_tag_name("tc") && is_w(a));
            if notes || in_cell {
                edits.push((p.range(), ""));
            } else {
                let at = qname_end(xml, p.range().start);
                edits.push((at..at, MARK));
            }
            skip_inside = Some(p.range());
            continue;
        }
        // A shown paragraph: cut its own hidden text runs (not those of a
        // paragraph nested in it, a text box's, read on their own).
        for run in p.descendants().filter(|n| {
            n.has_tag_name("r")
                && is_w(*n)
                && n.ancestors()
                    .skip(1)
                    .find(|a| a.has_tag_name("p") && is_w(*a))
                    == Some(p)
        }) {
            let carries_object = run.descendants().any(|n| {
                matches!(
                    n.tag_name().name(),
                    "drawing" | "pict" | "object" | "oMath" | "AlternateContent" | "txbxContent"
                )
            });
            if !carries_object && styles.run_hidden(run, pstyle) {
                edits.push((run.range(), ""));
            }
        }
    }
    if edits.is_empty() {
        return Cow::Borrowed(xml);
    }
    edits.sort_by_key(|(r, _)| (r.start, r.end));
    let mut out = String::with_capacity(xml.len());
    let mut pos = 0;
    for (range, replacement) in edits {
        if range.start < pos {
            continue; // defensive: a cut inside one already made
        }
        out.push_str(&xml[pos..range.start]);
        out.push_str(replacement);
        pos = range.end;
    }
    out.push_str(&xml[pos..]);
    Cow::Owned(out)
}

/// A WordprocessingML element (the main namespace, or a namespace-less
/// test fixture).
fn is_w(n: Node) -> bool {
    matches!(
        n.tag_name().namespace(),
        None | Some("http://schemas.openxmlformats.org/wordprocessingml/2006/main")
            | Some("http://purl.oclc.org/ooxml/wordprocessingml/main")
    )
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(name) && is_w(*c))
}

/// An attribute by local name, whatever its prefix.
fn local_attr<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attributes().find(|a| a.name() == name).map(|a| a.value())
}

/// The offset just past an element's qualified name, `start` being its `<`.
fn qname_end(src: &str, start: usize) -> usize {
    let tag = &src[start + 1..];
    let len = tag
        .find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
        .unwrap_or(tag.len());
    start + 1 + len
}
