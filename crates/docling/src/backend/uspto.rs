//! USPTO patent XML backend (core) — a port of the modern
//! `us-patent-application`/`us-patent-grant` (v4x) path of docling's
//! `PatentUsptoDocumentBackend`. Emits the invention title (#), the ABSTRACT
//! (###) + text, the description's `<heading>`s (by their `level`) and `<p>`s,
//! and the CLAIMS, including CALS `<table>`s (ported from docling's `XmlTable`).
//! Older schemas (pap-v15, PATDOC, the legacy APS text format) and maths are
//! out of scope for the core.

use std::borrow::Cow;

use roxmltree::{Document, Node as XmlNode, ParsingOptions};

use crate::backend::markdown::escape_text;
use crate::backend::uspto_entities::NAMED_ENTITIES;
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;
use docling_core::{DoclingDocument, Node, Table, TableStructure};

pub struct UsptoBackend;

impl DeclarativeBackend for UsptoBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let raw = source.text()?;
        let xml = resolve_named_entities(raw);
        let opts = ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        let dom = Document::parse_with_options(&xml, opts)
            .map_err(|e| ConversionError::Parse(format!("uspto: {e}")))?;
        let mut doc = DoclingDocument::new(&source.name);

        if let Some(title) = dom
            .descendants()
            .find(|n| n.has_tag_name("invention-title"))
            .map(node_text)
            .filter(|s| !s.is_empty())
        {
            doc.push(Node::Heading {
                level: 1,
                text: escape_text(&title),
            });
        }

        if let Some(abs) = dom.descendants().find(|n| n.has_tag_name("abstract")) {
            let paras = paragraphs(abs);
            if !paras.is_empty() {
                doc.push(Node::Heading {
                    level: 3,
                    text: "ABSTRACT".into(),
                });
                // docling emits the abstract as a single text item — its
                // paragraphs (with any chemistry-drawing `<p>` dropped as empty)
                // are joined into one, not split per `<p>`.
                doc.push(Node::Paragraph {
                    text: escape_text(&paras.join(" ")),
                });
            }
        }

        if let Some(desc) = dom.descendants().find(|n| n.has_tag_name("description")) {
            walk_description(desc, &mut doc);
        }

        if let Some(claims) = dom.descendants().find(|n| n.has_tag_name("claims")) {
            doc.push(Node::Heading {
                level: 3,
                text: "CLAIMS".into(),
            });
            for claim in claims.children().filter(|c| c.has_tag_name("claim")) {
                for ct in claim.children().filter(|c| c.has_tag_name("claim-text")) {
                    let t = node_text(ct);
                    if !t.is_empty() {
                        doc.push(Node::Paragraph {
                            text: escape_text(&t),
                        });
                    }
                }
            }
        }
        Ok(doc)
    }
}

/// Walk `<description>`: `<heading level="N">` → a heading (`#`×(N+2)), `<p>` →
/// a paragraph; recurse into other containers.
fn walk_description(node: XmlNode, doc: &mut DoclingDocument) {
    for child in node.children().filter(XmlNode::is_element) {
        match child.tag_name().name() {
            "heading" => {
                let level = child
                    .attribute("level")
                    .and_then(|v| v.parse::<u8>().ok())
                    .unwrap_or(1);
                let t = node_text(child);
                if !t.is_empty() {
                    doc.push(Node::Heading {
                        level: level + 2,
                        text: escape_text(&t),
                    });
                }
            }
            "p" => {
                // A `<p>` may wrap `<tables>` (USPTO nests them inside a
                // paragraph). docling adds a table at each `<table>` position and
                // the wrapping paragraph emits no text of its own.
                if child.descendants().any(|n| n.has_tag_name("table")) {
                    push_tables(child, doc);
                } else {
                    let t = node_text(child);
                    if !t.is_empty() {
                        doc.push(Node::Paragraph {
                            text: escape_text(&t),
                        });
                    }
                }
            }
            "maths" => {}
            "tables" | "table" => push_tables(child, doc),
            _ => walk_description(child, doc),
        }
    }
}

// ---------------------------------------------------------------------------
// CALS table parsing — a port of docling's `XmlTable._parse_table` /
// `_create_tg_range`. USPTO tables are CALS (`<tgroup>/<colspec>/<thead>/
// <tbody>/<row>/<entry>`); several `<tgroup>`s with differing column counts are
// merged into one grid `ncols_max` wide, spanned cell text is replicated across
// the physical columns, and empty rows are dropped. The DocLang OTSL overlay
// (`TableStructure`) records header rows and horizontal-span continuations.
// ---------------------------------------------------------------------------

/// Emit a [`Node::Table`] for every `<table>` element in `node`'s subtree
/// (or `node` itself), in document order — mirroring docling, which records one
/// table per `<table>` tag it encounters.
fn push_tables(node: XmlNode, doc: &mut DoclingDocument) {
    let tables: Vec<XmlNode> = if node.has_tag_name("table") {
        vec![node]
    } else {
        node.descendants()
            .filter(|n| n.has_tag_name("table"))
            .collect()
    };
    for tn in tables {
        if let Some(t) = parse_table(tn) {
            doc.push(Node::Table(t));
        }
    }
}

/// Parse a CALS `<colspec>` width (`"42pt"`, `"24.47mm"`) to a number.
fn parse_colwidth(s: &str) -> f64 {
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    cleaned.parse::<f64>().unwrap_or(0.0)
}

/// Python `str.isnumeric()` for the ASCII digit case we care about.
fn as_index(s: &str) -> Option<i64> {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        s.parse::<i64>().ok()
    } else {
        None
    }
}

fn sorted_unique(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v.dedup();
    v
}

/// Build the per-tgroup unified column range mapping (`cell_offst`).
/// `tgs[itg]` = that tgroup's colspec widths. Returns one `cell_offst` vector
/// per tgroup, or `None` if the offsets are inconsistent (docling bails to an
/// empty table).
fn create_tg_range(tgs: &[Vec<f64>]) -> Option<Vec<Vec<i64>>> {
    if tgs.is_empty() {
        return Some(Vec::new());
    }
    // Cumulative column boundary offsets per tgroup (len = ncols + 1).
    let mut offsets: Vec<Vec<f64>> = Vec::with_capacity(tgs.len());
    for cws in tgs {
        let mut off = Vec::with_capacity(cws.len() + 1);
        let mut o = 0.0;
        for &cw in cws {
            off.push(o);
            o += cw;
        }
        off.push(o);
        offsets.push(off);
    }
    // Unified boundary set across all tgroups; zero-width columns are injected
    // back as duplicate boundaries (docling does not de-dupe that final sort).
    let mut min_off: Vec<f64> = offsets[0].clone();
    let mut offset_w0: Vec<f64> = Vec::new();
    for (itg, cws) in tgs.iter().enumerate() {
        for (ic, &cw) in cws.iter().enumerate() {
            if cw == 0.0 {
                offset_w0.push(offsets[itg][ic]);
            }
        }
        let union: Vec<f64> = offsets[itg].iter().chain(min_off.iter()).copied().collect();
        min_off = sorted_unique(union);
    }
    offset_w0 = sorted_unique(offset_w0);
    let mut combined: Vec<f64> = min_off.iter().chain(offset_w0.iter()).copied().collect();
    combined.sort_by(|a, b| a.partial_cmp(b).unwrap());
    min_off = combined;

    // Map each tgroup's columns onto the unified grid.
    let mut result: Vec<Vec<i64>> = Vec::with_capacity(tgs.len());
    for col_offset in &offsets {
        let mut cell_offst: Vec<i64> = vec![0];
        let mut i = 1usize;
        let mut range_: i64 = 1;
        for min_i in 1..min_off.len() {
            let min_offst = min_off[min_i];
            let offst = *col_offset.get(i)?;
            if min_offst == offst {
                if col_offset.len() == i + 1 && min_off.len() > min_i + 1 {
                    range_ += 1;
                } else {
                    cell_offst.push(cell_offst[cell_offst.len() - 1] + range_);
                    range_ = 1;
                    i += 1;
                }
            } else if min_offst < offst {
                range_ += 1;
            } else {
                return None;
            }
        }
        result.push(cell_offst);
    }
    Some(result)
}

/// A cell's plain text — docling uses BeautifulSoup `get_text().strip()` for
/// table entries (no super/subscript translation, unlike paragraph text).
fn cell_text(node: XmlNode) -> String {
    let mut s = String::new();
    for d in node.descendants() {
        if d.is_text() {
            if let Some(t) = d.text() {
                s.push_str(t);
            }
        }
    }
    s.trim().to_string()
}

/// Port of `XmlTable._parse_table`: CALS `<table>` → a [`Table`] with an OTSL
/// [`TableStructure`] overlay. Returns `None` for a broken/empty table.
fn parse_table(table: XmlNode) -> Option<Table> {
    let tgroups: Vec<XmlNode> = table
        .children()
        .filter(|n| n.has_tag_name("tgroup"))
        .collect();
    let tgs_cw: Vec<Vec<f64>> = tgroups
        .iter()
        .map(|tg| {
            tg.children()
                .filter(|n| n.has_tag_name("colspec"))
                .map(|cs| cs.attribute("colwidth").map(parse_colwidth).unwrap_or(0.0))
                .collect()
        })
        .collect();

    let tgs_range = create_tg_range(&tgs_cw)?;
    if tgs_range.is_empty() {
        return None;
    }
    let ncols_max = tgs_cw.iter().map(Vec::len).max().unwrap_or(0);
    if ncols_max == 0 {
        return None;
    }

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut header_row: Vec<bool> = Vec::new();
    let mut col_continuation: Vec<Vec<bool>> = Vec::new();

    for (itg, tg) in tgroups.iter().enumerate() {
        let cell_offst = &tgs_range[itg];
        let row_nodes: Vec<XmlNode> = tg
            .descendants()
            .filter(|n| n.has_tag_name("row") || n.has_tag_name("tr"))
            .collect();
        for row_sec in row_nodes {
            let is_header = row_sec.parent().is_some_and(|p| p.has_tag_name("thead"));
            let entries: Vec<XmlNode> = row_sec
                .children()
                .filter(|n| n.has_tag_name("entry") || n.has_tag_name("td"))
                .collect();

            let mut row_text: Vec<String> = Vec::new();
            let mut row_cont: Vec<bool> = Vec::new();
            let mut ncols = 0usize;
            let mut is_row_empty = true;
            let mut wrong_nbr_cols = false;

            for (ientry, entry) in entries.iter().enumerate() {
                let text = cell_text(*entry);
                let start = entry
                    .attribute("namest")
                    .and_then(as_index)
                    .unwrap_or(ientry as i64 + 1);
                let (end, shift) = match entry.attribute("nameend").and_then(as_index) {
                    Some(e) => (e, 0),
                    None => (ientry as i64 + 2, 1),
                };
                if end > cell_offst.len() as i64 || start < 1 {
                    wrong_nbr_cols = true;
                    break;
                }
                let r0 = cell_offst[(start - 1) as usize];
                let r1 = cell_offst[(end - 1) as usize] - shift;
                if !text.is_empty() {
                    is_row_empty = false;
                }
                let mut irep = r0;
                let mut first = true;
                while irep <= r1 {
                    ncols += 1;
                    row_text.push(text.clone());
                    row_cont.push(!first);
                    first = false;
                    irep += 1;
                }
            }

            if wrong_nbr_cols {
                row_text.clear();
                row_cont.clear();
                ncols = 0;
            }
            while ncols < ncols_max {
                row_text.push(String::new());
                row_cont.push(false);
                ncols += 1;
            }

            if !is_row_empty {
                rows.push(row_text);
                col_continuation.push(row_cont);
                header_row.push(is_header);
            }
        }
    }

    if rows.is_empty() {
        return None;
    }
    Some(Table {
        rows,
        location: None,
        structure: Some(TableStructure {
            header_row,
            col_continuation,
        }),
    })
}

/// Each `<p>` descendant's normalized text.
fn paragraphs(node: XmlNode) -> Vec<String> {
    node.descendants()
        .filter(|n| n.has_tag_name("p"))
        .map(node_text)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Recursive normalized text of a node, skipping `<maths>` (docling drops formulas).
fn node_text(node: XmlNode) -> String {
    let mut s = String::new();
    raw_text(node, &mut s);
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn raw_text(node: XmlNode, out: &mut String) {
    // Walk every child in order. Text is captured from text-node children
    // directly (not via node.text()/tail shortcuts), so text following a
    // processing instruction or comment — e.g. the leading "R" in
    // `<?in-line-formulae?>R<sup>1</sup>—CO…` — is not dropped.
    for child in node.children() {
        if child.is_text() {
            if let Some(t) = child.text() {
                out.push_str(&t.replace('\n', " "));
            }
        } else if child.is_element() {
            match child.tag_name().name() {
                // <sup>/<sub> digits and signs render as Unicode super/subscript.
                "sup" | "sub" => {
                    let mut inner = String::new();
                    raw_text(child, &mut inner);
                    let sup = child.tag_name().name() == "sup";
                    out.extend(inner.chars().map(|c| script_char(c, sup)));
                }
                "maths" => {}
                _ => raw_text(child, out),
            }
        }
    }
}

/// Map a digit/sign/letter to its Unicode super- or subscript form (else
/// unchanged) — matching docling's `style_html` translation tables exactly.
fn script_char(c: char, sup: bool) -> char {
    if sup {
        match c {
            '0' => '⁰',
            '1' => '¹',
            '2' => '²',
            '3' => '³',
            '4' => '⁴',
            '5' => '⁵',
            '6' => '⁶',
            '7' => '⁷',
            '8' => '⁸',
            '9' => '⁹',
            '+' => '⁺',
            '-' | '−' => '⁻',
            '=' => '⁼',
            '(' => '⁽',
            ')' => '⁾',
            'a' => 'ª',
            'o' => 'º',
            'i' => 'ⁱ',
            'n' => 'ⁿ',
            _ => c,
        }
    } else {
        match c {
            '0' => '₀',
            '1' => '₁',
            '2' => '₂',
            '3' => '₃',
            '4' => '₄',
            '5' => '₅',
            '6' => '₆',
            '7' => '₇',
            '8' => '₈',
            '9' => '₉',
            '+' => '₊',
            '-' | '−' => '₋',
            '=' => '₌',
            '(' => '₍',
            ')' => '₎',
            'a' => 'ₐ',
            'e' => 'ₑ',
            'o' => 'ₒ',
            'x' => 'ₓ',
            _ => c,
        }
    }
}

/// Resolve non-predefined named character references (`&trade;`, `&agr;`,
/// `&lsqb;`, …) into their literal characters so roxmltree — which only knows
/// the five XML built-ins and internally declared entities — can parse legacy
/// USPTO SGML documents. Mirrors docling's `skippedEntity` handling: the ISO
/// 8879 Greek names fold onto their HTML5 counterparts inside the generated
/// table, recognized entities expand, and unrecognized ones are dropped.
///
/// The XML built-ins (`amp`/`lt`/`gt`/`quot`/`apos`) and numeric references
/// (`&#…;`) are left untouched for the parser to resolve. Entity references
/// whose name uses characters outside `[A-Za-z0-9]` (the unparsed-graphics
/// `NDATA` entities USPTO declares in its internal subset) are also dropped —
/// they are illegal in element content and would abort the parse.
fn resolve_named_entities(xml: &str) -> Cow<'_, str> {
    if !xml.contains('&') {
        return Cow::Borrowed(xml);
    }
    let mut out = String::with_capacity(xml.len());
    let mut i = 0;
    while let Some(rel) = xml[i..].find('&') {
        let amp = i + rel;
        out.push_str(&xml[i..amp]);
        // Find the terminating ';' within a bounded window (entity names are short).
        let end = xml[amp + 1..]
            .char_indices()
            .take(64)
            .find(|&(_, c)| c == ';')
            .map(|(off, _)| amp + 1 + off);
        let Some(semi) = end else {
            out.push('&');
            i = amp + 1;
            continue;
        };
        let name = &xml[amp + 1..semi];
        i = semi + 1;
        // Numeric references and the XML built-ins pass through verbatim.
        if name.starts_with('#') || matches!(name, "amp" | "lt" | "gt" | "quot" | "apos") {
            out.push('&');
            out.push_str(name);
            out.push(';');
            continue;
        }
        // Only plain-alphanumeric names can be table entities; anything else is
        // a declared graphics entity — drop it (docling skips those too).
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric()) {
            continue;
        }
        // Recognized names expand; unrecognized ones are dropped (docling skips
        // them too).
        if let Ok(idx) = NAMED_ENTITIES.binary_search_by(|&(n, _)| n.cmp(name)) {
            push_xml_escaped(&mut out, NAMED_ENTITIES[idx].1);
        }
    }
    out.push_str(&xml[i..]);
    Cow::Owned(out)
}

/// Append `s`, re-escaping the three characters that would otherwise disturb
/// the surrounding XML once this string is fed back to the parser.
fn push_xml_escaped(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::InputFormat;

    #[test]
    fn resolves_iso_and_html_entities_and_drops_unknown() {
        assert_eq!(resolve_named_entities("a&trade;b"), "a\u{2122}b");
        assert_eq!(resolve_named_entities("x&agr;y"), "x\u{3b1}y"); // ISO 8879 alpha
        assert_eq!(resolve_named_entities("p&lsqb;q&rsqb;"), "p[q]");
        assert_eq!(
            resolve_named_entities("keep &amp; and &#65;"),
            "keep &amp; and &#65;"
        );
        assert_eq!(resolve_named_entities("drop&zzznope;it"), "dropit");
        assert_eq!(resolve_named_entities("amp&AMP;ersand"), "amp&amp;ersand");
        // Declared NDATA graphics entity (dot in the name) — dropped.
        assert_eq!(resolve_named_entities("g&US001.TIF;h"), "gh");
        assert_eq!(
            resolve_named_entities("no entities here"),
            "no entities here"
        );
    }

    #[test]
    fn title_abstract_headings_and_scripts() {
        let xml = r#"<us-patent-application>
            <us-bibliographic-data-application>
              <invention-title>A Device</invention-title>
            </us-bibliographic-data-application>
            <abstract><p>An H<sub>2</sub>O cell at 10<sup>-3</sup>.</p></abstract>
            <description>
              <heading level="1">BACKGROUND</heading>
              <p>Body of NO<sub>3</sub><sup>-</sup>.</p>
              <heading level="2">Detail</heading>
            </description>
          </us-patent-application>"#;
        let src = SourceDocument::from_bytes("p", InputFormat::XmlUspto, xml.as_bytes().to_vec());
        let md = UsptoBackend.convert(&src).unwrap().export_to_markdown();
        assert!(
            md.starts_with(
                "# A Device\n\n### ABSTRACT\n\nAn H₂O cell at 10⁻³.\n\n### BACKGROUND\n\nBody of NO₃⁻.\n\n#### Detail"
            ),
            "got:\n{md}"
        );
    }

    #[test]
    fn keeps_text_following_a_processing_instruction() {
        // The leading run before an <?in-line-formulae?> PI is the PI's tail;
        // it must not be dropped (docling keeps "R¹—CO", not "¹—CO").
        let xml = r#"<us-patent-application>
            <description>
              <p><?in-line-formulae description="In-line Formulae" end="lead"?>R<sup>1</sup>&#x2014;CO</p>
            </description>
          </us-patent-application>"#;
        let src = SourceDocument::from_bytes("p", InputFormat::XmlUspto, xml.as_bytes().to_vec());
        let md = UsptoBackend.convert(&src).unwrap().export_to_markdown();
        assert!(md.contains("R¹—CO"), "got:\n{md}");
    }

    fn dclx_of(xml: &str) -> String {
        let src = SourceDocument::from_bytes("p", InputFormat::XmlUspto, xml.as_bytes().to_vec());
        UsptoBackend.convert(&src).unwrap().export_to_doclang()
    }

    #[test]
    fn cals_table_spans_and_header() {
        // A `<p>`-wrapped CALS table: `namest`/`nameend` span → `<ched/>`+`<lcel/>`,
        // thead → header, empty `<entry/>` → `<ecel/>`, empty rows dropped.
        let xml = r#"<us-patent-application><description><p><tables><table>
            <tgroup cols="3">
              <colspec colname="1" colwidth="40pt"/>
              <colspec colname="2" colwidth="40pt"/>
              <colspec colname="3" colwidth="40pt"/>
              <thead>
                <row><entry namest="1" nameend="3">Title</entry></row>
              </thead>
              <tbody>
                <row><entry>a</entry><entry>b</entry><entry>c</entry></row>
              </tbody>
            </tgroup>
          </table></tables></p></description></us-patent-application>"#;
        let dclx = dclx_of(xml);
        // Collapse indentation to a token stream for a layout-robust check.
        let toks: Vec<&str> = dclx.split_whitespace().collect();
        let joined = toks.join(" ");
        assert!(
            joined.contains("<ched/> Title <lcel/> <lcel/> <nl/>"),
            "spanning header not <ched/>+<lcel/>×2:\n{dclx}"
        );
        assert!(
            joined.contains("<fcel/> a <fcel/> b <fcel/> c <nl/>"),
            "data row wrong:\n{dclx}"
        );
    }

    #[test]
    fn cals_table_drops_empty_rows_and_reads_scripts_as_plain() {
        // An all-empty rule row is dropped; sup/sub in a cell stay plain text
        // (docling uses get_text() for cells, no unicode script translation).
        let xml = r#"<us-patent-application><description><p><tables><table>
            <tgroup cols="2">
              <colspec colname="1" colwidth="40pt"/>
              <colspec colname="2" colwidth="40pt"/>
              <tbody>
                <row><entry/><entry/></row>
                <row><entry>m<sup>2</sup></entry><entry>x</entry></row>
              </tbody>
            </tgroup>
          </table></tables></p></description></us-patent-application>"#;
        let dclx = dclx_of(xml);
        // one data row only (empty row dropped) -> exactly one <nl/> inside the table
        let table = dclx.split("<table>").nth(1).unwrap();
        assert_eq!(
            table.matches("<nl/>").count(),
            1,
            "empty row not dropped:\n{dclx}"
        );
        assert!(
            dclx.contains("m2"),
            "cell script converted (should be plain):\n{dclx}"
        );
    }

    #[test]
    fn abstract_paragraphs_join_into_one_text() {
        // docling emits the abstract as a single text item; a chemistry-drawing
        // <p> in the middle is dropped, the surrounding text stays one paragraph.
        let xml = r#"<us-patent-application>
            <abstract>
              <p>The invention relates to compounds of the formula (I)</p>
              <p><chemistry><img file="C00001.TIF"/></chemistry></p>
              <p>in which X has the meaning given above.</p>
            </abstract>
          </us-patent-application>"#;
        let src = SourceDocument::from_bytes("p", InputFormat::XmlUspto, xml.as_bytes().to_vec());
        let md = UsptoBackend.convert(&src).unwrap().export_to_markdown();
        assert!(
            md.contains(
                "### ABSTRACT\n\nThe invention relates to compounds of the formula (I) in which X has the meaning given above."
            ),
            "got:\n{md}"
        );
    }
}
