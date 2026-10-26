//! docling's DOCX fixes from 2.130–2.142 that upstream pins with unit tests
//! only (no groundtruth file to mirror): the tests are ported here, each
//! against both walks — the flat `docx.rs` chain through its Markdown, the
//! `docx_tree.rs` item tree through the parents and markers the JSON writes.

use super::DeclarativeBackend;
use crate::{InputFormat, SourceDocument};
use docling_core::tree::{ItemTree, TreeKind};
use docling_core::DoclingDocument;
use std::io::Write;

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const HYPERLINK: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";

fn package(parts: &[(&str, String)]) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, content) in parts {
        zw.start_file(*name, opts).unwrap();
        zw.write_all(content.as_bytes()).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

fn convert(bytes: Vec<u8>) -> DoclingDocument {
    let src = SourceDocument::from_bytes("t.docx", InputFormat::Docx, bytes);
    super::DocxBackend::default()
        .convert(&src)
        .expect("converts")
}

fn tree(doc: &DoclingDocument) -> &ItemTree {
    doc.tree
        .as_ref()
        .expect("the DOCX backend builds an item tree")
}

/// The flat chain's Markdown — checked against Python docling 2.142.1's
/// `export_to_markdown()` on the same documents built with python-docx.
fn md(doc: &DoclingDocument) -> String {
    doc.export_to_markdown().trim_end().to_string()
}

fn text_of(kind: &TreeKind) -> &str {
    match kind {
        TreeKind::Text { text, .. } | TreeKind::Code { text, .. } => text,
        _ => "",
    }
}

/// The live tree item whose text is `t`.
fn item(tree: &ItemTree, t: &str) -> usize {
    tree.items
        .iter()
        .position(|it| text_of(&it.kind) == t && !it.deleted)
        .unwrap_or_else(|| panic!("no item {t:?}"))
}

fn parent(tree: &ItemTree, i: usize) -> Option<usize> {
    tree.items[i].parent
}

fn marker(tree: &ItemTree, t: &str) -> String {
    match &tree.items[item(tree, t)].kind {
        TreeKind::Text {
            list: Some(meta), ..
        } => meta.marker.clone(),
        other => panic!("{t:?} is no list item: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// docling#4465: a list indented under an open list nests under its item.

/// python-docx's default template, cut to what the list tests read: the
/// "List Bullet" / "List Number" styles each link their own `numId` at
/// `w:ilvl` 0, and the levels differ only by their left indentation (360
/// twips a level). `extra` is appended to the styles.
fn list_styles(extra: &str) -> String {
    let style = |id: &str, name: &str, num: u32| {
        format!(
            r#"<w:style w:type="paragraph" w:styleId="{id}"><w:name w:val="{name}"/><w:basedOn w:val="Normal"/><w:pPr><w:numPr><w:numId w:val="{num}"/></w:numPr></w:pPr></w:style>"#
        )
    };
    format!(
        r#"<w:styles xmlns:w="{W}"><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/></w:style>{}{}{}{}{}{extra}</w:styles>"#,
        style("ListBullet", "List Bullet", 1),
        style("ListBullet2", "List Bullet 2", 2),
        style("ListNumber", "List Number", 3),
        style("ListNumber2", "List Number 2", 4),
        style("ListNumber3", "List Number 3", 5),
    )
}

/// One `abstractNum` + `num` per list style; `indents[k]` is the
/// `w:ind w:left` of `numId` k + 1's level 0 (`None` = no `w:ind`).
fn list_numbering(indents: [Option<u32>; 5]) -> String {
    let fmts = ["bullet", "bullet", "decimal", "decimal", "decimal"];
    let mut out = format!(r#"<w:numbering xmlns:w="{W}">"#);
    for (k, (fmt, ind)) in fmts.iter().zip(indents).enumerate() {
        let text = if *fmt == "bullet" { "&#8226;" } else { "%1." };
        let ind = ind
            .map(|l| format!(r#"<w:pPr><w:ind w:left="{l}" w:hanging="360"/></w:pPr>"#))
            .unwrap_or_default();
        out.push_str(&format!(
            r#"<w:abstractNum w:abstractNumId="{k}"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="{fmt}"/><w:lvlText w:val="{text}"/>{ind}</w:lvl></w:abstractNum>"#
        ));
    }
    for k in 0..fmts.len() {
        out.push_str(&format!(
            r#"<w:num w:numId="{}"><w:abstractNumId w:val="{k}"/></w:num>"#,
            k + 1
        ));
    }
    out.push_str("</w:numbering>");
    out
}

const STOCK_INDENTS: [Option<u32>; 5] = [Some(360), Some(720), Some(360), Some(720), Some(1080)];

fn styled(text: &str, style: &str) -> String {
    let run = if text.is_empty() {
        String::new()
    } else {
        format!("<w:r><w:t>{text}</w:t></w:r>")
    };
    format!(r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr>{run}</w:p>"#)
}

fn list_docx(body: &str, styles: String, numbering: String) -> DoclingDocument {
    convert(package(&[
        (
            "word/document.xml",
            format!(r#"<w:document xmlns:w="{W}"><w:body>{body}<w:sectPr/></w:body></w:document>"#),
        ),
        ("word/styles.xml", styles),
        ("word/numbering.xml", numbering),
    ]))
}

fn styled_paragraphs(paragraphs: &[(&str, &str)]) -> DoclingDocument {
    let body: String = paragraphs.iter().map(|(t, s)| styled(t, s)).collect();
    list_docx(&body, list_styles(""), list_numbering(STOCK_INDENTS))
}

/// `test_list_styles_nest_by_indentation`.
#[test]
fn list_styles_nest_by_indentation() {
    let doc = styled_paragraphs(&[
        ("apple", "ListBullet"),
        ("banana", "ListBullet"),
        ("banana split", "ListBullet2"),
        ("banana bread", "ListBullet2"),
        ("cherry", "ListBullet"),
        ("first", "ListNumber"),
        ("first-a", "ListNumber2"),
        ("second", "ListNumber"),
    ]);
    assert_eq!(
        md(&doc),
        "- apple\n- banana\n    - banana split\n    - banana bread\n- cherry\n\n\
         1. first\n    1. first-a\n2. second"
    );
    let t = tree(&doc);
    let bullets = parent(t, item(t, "banana")).unwrap();
    let sub_bullets = parent(t, item(t, "banana split")).unwrap();
    // The sub-list hangs from the item above it (docling#4863), which hangs
    // from the outer list.
    assert_eq!(parent(t, sub_bullets), Some(item(t, "banana")));
    assert_eq!(parent(t, item(t, "banana")), Some(bullets));
    assert_eq!(parent(t, item(t, "banana bread")), Some(sub_bullets));
    assert_eq!(parent(t, item(t, "cherry")), Some(bullets));
    let numbers = parent(t, item(t, "first")).unwrap();
    let sub_numbers = parent(t, item(t, "first-a")).unwrap();
    assert_eq!(parent(t, sub_numbers), Some(item(t, "first")));
    assert_eq!(parent(t, item(t, "second")), Some(numbers));
    assert_eq!(marker(t, "second"), "2.");
}

/// `test_lists_at_the_same_indentation_stay_siblings`.
#[test]
fn lists_at_the_same_indentation_stay_siblings() {
    let doc = styled_paragraphs(&[
        ("bullet", "ListBullet"),
        ("bullet-a", "ListBullet2"),
        ("bullet-1", "ListNumber2"),
        ("number", "ListNumber"),
    ]);
    let t = tree(&doc);
    let bullets = parent(t, item(t, "bullet")).unwrap();
    let sub_bullets = parent(t, item(t, "bullet-a")).unwrap();
    let sub_numbers = parent(t, item(t, "bullet-1")).unwrap();
    assert_eq!(parent(t, sub_bullets), Some(item(t, "bullet")));
    assert_eq!(parent(t, sub_numbers), Some(item(t, "bullet")));
    assert_ne!(sub_numbers, sub_bullets);
    let numbers = parent(t, item(t, "number")).unwrap();
    assert_ne!(numbers, bullets);
    assert_eq!(parent(t, numbers), None);
    assert_eq!(parent(t, bullets), None);
    assert_eq!(
        md(&doc),
        "- bullet\n    - bullet-a\n    1. bullet-1\n\n1. number"
    );
}

/// `test_list_styles_return_to_the_outer_list`.
#[test]
fn list_styles_return_to_the_outer_list() {
    let doc = styled_paragraphs(&[
        ("one", "ListNumber"),
        ("one-a", "ListNumber2"),
        ("one-a-i", "ListNumber3"),
        ("two", "ListNumber"),
        ("two-a", "ListNumber2"),
        ("three", "ListNumber"),
    ]);
    let t = tree(&doc);
    let outer = parent(t, item(t, "one"));
    assert_eq!(parent(t, item(t, "two")), outer);
    assert_eq!(parent(t, item(t, "three")), outer);
    assert_eq!(
        ["one", "two", "three"].map(|x| marker(t, x)),
        ["1.", "2.", "3."]
    );
    let one_a_group = parent(t, item(t, "one-a"));
    let deepest = parent(t, item(t, "one-a-i")).unwrap();
    assert_eq!(parent(t, deepest), Some(item(t, "one-a")));
    assert_eq!(parent(t, one_a_group.unwrap()), Some(item(t, "one")));
    let second_sub_list = parent(t, item(t, "two-a")).unwrap();
    assert_eq!(parent(t, second_sub_list), Some(item(t, "two")));
    assert_ne!(Some(second_sub_list), one_a_group);
    // "two-a" resumes Word's "List Number 2" list (one `numId`), so its
    // counter goes on, as Word numbers it.
    assert_eq!(marker(t, "two-a"), "2.");
    assert_eq!(
        md(&doc),
        "1. one\n    1. one-a\n        1. one-a-i\n2. two\n    2. two-a\n3. three"
    );
}

/// `test_blank_paragraph_closes_a_nested_list_style`: the group kept for
/// reuse across the blank is the outer list's — an outer item after it
/// resumes the outer list, a nested item is not attached to it.
#[test]
fn blank_paragraph_closes_a_nested_list_style() {
    let convert_with = |last: (&str, &str)| {
        let body: String = [
            ("Section", "Heading1"),
            ("first", "ListNumber"),
            ("first-a", "ListNumber2"),
            ("", "Normal"),
            last,
        ]
        .iter()
        .map(|(t, s)| styled(t, s))
        .collect();
        list_docx(&body, list_styles(""), list_numbering(STOCK_INDENTS))
    };
    let doc = convert_with(("second", "ListNumber"));
    let t = tree(&doc);
    assert_eq!(parent(t, item(t, "second")), parent(t, item(t, "first")));
    assert_eq!(marker(t, "second"), "2.");
    assert_eq!(
        md(&doc),
        "## Section\n\n1. first\n    1. first-a\n2. second"
    );

    let doc = convert_with(("first-b", "ListNumber2"));
    let t = tree(&doc);
    assert_ne!(parent(t, item(t, "first-b")), parent(t, item(t, "first")));
    assert_eq!(
        md(&doc),
        "## Section\n\n1. first\n    1. first-a\n\n2. first-b"
    );
}

/// `test_list_nesting_follows_word_indentation_precedence`: the paragraph's
/// own `w:ind` (spelled `w:start`) overrides its numbering level's; when
/// neither sets one, the style's `basedOn` chain decides.
#[test]
fn list_nesting_follows_word_indentation_precedence() {
    // "List Number 2" inherits its indentation from a base style once its
    // numbering level no longer sets one.
    let styles = list_styles(
        r#"<w:style w:type="paragraph" w:styleId="IndentedBase"><w:name w:val="Indented Base"/><w:pPr><w:ind w:left="720"/></w:pPr></w:style>"#,
    )
    .replace(
        r#"<w:style w:type="paragraph" w:styleId="ListNumber2"><w:name w:val="List Number 2"/><w:basedOn w:val="Normal"/>"#,
        r#"<w:style w:type="paragraph" w:styleId="ListNumber2"><w:name w:val="List Number 2"/><w:basedOn w:val="IndentedBase"/>"#,
    );
    let numbering = list_numbering([Some(360), Some(720), Some(360), None, Some(1080)]);
    let body = format!(
        "{}{}{}{}",
        styled("outer", "ListBullet"),
        styled("by style", "ListNumber2"),
        styled("back", "ListBullet"),
        // "List Number" is numbered at the "List Bullet" indentation.
        r#"<w:p><w:pPr><w:pStyle w:val="ListNumber"/><w:ind w:start="720"/></w:pPr><w:r><w:t>by paragraph</w:t></w:r></w:p>"#,
    );
    let doc = list_docx(&body, styles, numbering);
    let t = tree(&doc);
    let outer = parent(t, item(t, "outer"));
    assert_eq!(parent(t, item(t, "back")), outer);
    let by_style = parent(t, item(t, "by style")).unwrap();
    assert_eq!(parent(t, parent(t, by_style).unwrap()), outer);
    let by_paragraph = parent(t, item(t, "by paragraph")).unwrap();
    assert_eq!(parent(t, parent(t, by_paragraph).unwrap()), outer);
    assert_eq!(
        md(&doc),
        "- outer\n    1. by style\n- back\n    1. by paragraph"
    );
}

/// The same `numId` keeps nesting by `w:ilvl` alone, whatever the
/// indentation says: a regression net for the lists docling always read.
#[test]
fn one_numbering_still_nests_by_ilvl() {
    let numbering = format!(
        r#"<w:numbering xmlns:w="{W}"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:pPr><w:ind w:left="720"/></w:pPr></w:lvl><w:lvl w:ilvl="1"><w:numFmt w:val="decimal"/><w:lvlText w:val="%2."/><w:pPr><w:ind w:left="1440"/></w:pPr></w:lvl></w:abstractNum><w:num w:numId="7"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#
    );
    let item = |t: &str, ilvl: u8| {
        format!(
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="{ilvl}"/><w:numId w:val="7"/></w:numPr></w:pPr><w:r><w:t>{t}</w:t></w:r></w:p>"#
        )
    };
    let body = [item("a", 0), item("a1", 1), item("b", 0)].concat();
    let doc = list_docx(&body, list_styles(""), numbering);
    assert_eq!(md(&doc), "1. a\n    - 1.1. a1\n2. b");
}

// ---------------------------------------------------------------------------
// Inline content: footnotes (docling#4735), links (#4576, #4563/#4613), code.

/// A package around `body` with `rels` (extra `Relationship` elements) and
/// `parts` (extra package parts).
fn body_docx(body: &str, rels: &str, parts: &[(&str, String)]) -> DoclingDocument {
    let mut all = vec![
        (
            "word/document.xml",
            format!(
                r#"<w:document xmlns:w="{W}" xmlns:r="{R}"><w:body>{body}<w:sectPr/></w:body></w:document>"#
            ),
        ),
        (
            "word/_rels/document.xml.rels",
            format!(r#"<Relationships xmlns="{RELS}">{rels}</Relationships>"#),
        ),
    ];
    all.extend(parts.iter().map(|(n, c)| (*n, c.clone())));
    convert(package(&all))
}

/// `test_footnote_body_text_is_not_dropped_outside_plain_paragraphs`: a
/// note whose text sits in a tracked insertion, a content control or a
/// table is kept — the note body is read at any depth, through the inline
/// wrappers a body paragraph is read through.
#[test]
fn footnote_text_outside_plain_paragraphs_is_kept() {
    for (note_body, expected) in [
        (
            "<w:p><w:ins w:id='10' w:author='A' w:date='2024-01-02T03:04:05Z'>\
             <w:r><w:t>inserted footnote text</w:t></w:r></w:ins></w:p>",
            "inserted footnote text",
        ),
        (
            "<w:p><w:sdt><w:sdtPr/><w:sdtContent>\
             <w:r><w:t>sdt footnote text</w:t></w:r></w:sdtContent></w:sdt></w:p>",
            "sdt footnote text",
        ),
        (
            "<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w='2000'/></w:tblGrid>\
             <w:tr><w:tc><w:tcPr/><w:p><w:r><w:t>note table cell</w:t></w:r></w:p>\
             </w:tc></w:tr></w:tbl>",
            "note table cell",
        ),
    ] {
        let footnotes = format!(
            r#"<w:footnotes xmlns:w="{W}"><w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:id="2">{note_body}</w:footnote></w:footnotes>"#
        );
        let doc = body_docx(
            r#"<w:p><w:r><w:t>Body</w:t></w:r><w:r><w:footnoteReference w:id="2"/></w:r></w:p>"#,
            r#"<Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes" Target="footnotes.xml"/>"#,
            &[("word/footnotes.xml", footnotes)],
        );
        let t = tree(&doc);
        let note = item(t, expected);
        assert!(
            matches!(&t.items[note].kind, TreeKind::Text { label, .. } if label == "footnote"),
            "{expected:?}"
        );
        assert!(
            format!("{:?}", doc.nodes).contains(expected),
            "the flat chain lost {expected:?}"
        );
    }
}

/// `test_hyperlink_keeps_runs_nested_in_a_wrapper`: a link's runs may sit
/// in a tracked insertion, a smart tag, custom XML or a simple field inside
/// the link — python-docx's `Hyperlink.text` reads the direct `w:r` only.
#[test]
fn hyperlink_keeps_runs_nested_in_a_wrapper() {
    for wrapper in ["ins", "smartTag", "customXml", "fldSimple"] {
        let doc = body_docx(
            &format!(
                r#"<w:p><w:r><w:t xml:space="preserve">See </w:t></w:r><w:hyperlink r:id="rId1"><w:r><w:t xml:space="preserve">the </w:t></w:r><w:{wrapper}><w:r><w:t>docs</w:t></w:r></w:{wrapper}></w:hyperlink></w:p>"#
            ),
            &format!(
                r#"<Relationship Id="rId1" Type="{HYPERLINK}" Target="https://example.com/" TargetMode="External"/>"#
            ),
            &[],
        );
        let t = tree(&doc);
        let links: Vec<(&str, &str)> = t
            .items
            .iter()
            .filter_map(|it| match &it.kind {
                TreeKind::Text {
                    text,
                    hyperlink: Some(h),
                    ..
                } => Some((text.as_str(), h.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(links, [("the docs", "https://example.com/")], "{wrapper}");
        assert_eq!(
            md(&doc),
            "See [the docs](https://example.com/)",
            "{wrapper}"
        );
    }
}

/// `test_fragment_only_rel_does_not_crash_backend` (docling#4563/#4613): a
/// hyperlink whose relationship is a fragment-only target — dropped from
/// the package, as docling's `_sanitize_docx` drops it — keeps its text,
/// unlinked, in one text item with the text around it.
#[test]
fn dangling_fragment_hyperlink_keeps_its_text() {
    let doc = body_docx(
        r#"<w:p><w:r><w:t xml:space="preserve">Before </w:t></w:r><w:hyperlink r:id="rId999"><w:r><w:t>internal link</w:t></w:r></w:hyperlink><w:r><w:t xml:space="preserve"> after</w:t></w:r></w:p>"#,
        &format!(
            r##"<Relationship Id="rId999" Type="{HYPERLINK}" Target="#_Proc%C3%A9dures_sp%C3%A9ciales"/>"##
        ),
        &[],
    );
    let t = tree(&doc);
    let texts: Vec<&TreeKind> = t
        .items
        .iter()
        .filter(|it| matches!(it.kind, TreeKind::Text { .. }))
        .map(|it| &it.kind)
        .collect();
    assert_eq!(texts.len(), 1);
    assert!(matches!(
        texts[0],
        TreeKind::Text { text, hyperlink: None, .. } if text == "Before internal link after"
    ));
    assert_eq!(md(&doc), "Before internal link after");
}

/// `test_docx_code_call_pattern_is_bounded` (docling#4973).
#[test]
fn code_call_pattern_is_bounded() {
    use super::docx::is_code_call;
    assert!(is_code_call("x = foo(1, 2)"));
    assert!(is_code_call("set()"));
    assert!(!is_code_call("party(ies)"));
    for unit in ["a(", "a(,"] {
        let text = unit.repeat(64_000 / unit.len());
        let start = std::time::Instant::now();
        assert!(!is_code_call(&text));
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }
    // Argument lists past the bound no longer count as a call.
    assert!(!is_code_call(&format!("foo({}, 1)", "a".repeat(300))));
    // The bounds, at their edges, and the first run stopping at the first
    // code-like character.
    assert!(is_code_call(&format!("foo({}, 1)", "a".repeat(200))));
    assert!(is_code_call(&format!("foo(a,{})", "b".repeat(200))));
    assert!(!is_code_call(&format!("foo(a,{})", "b".repeat(201))));
    assert!(is_code_call("print(sys.argv)"));
    assert!(is_code_call("f( )"));
    assert!(!is_code_call("(1, 2)"));
}

// ---------------------------------------------------------------------------
// docling#4480: `use_outline_level_for_headings`.

/// A style carrying `w:outlineLvl` only for the table of contents is a
/// heading by default — how a localized heading style is found — and body
/// text with the option off; a style named a heading keeps its outline
/// level either way.
#[test]
fn outline_level_headings_follow_the_option() {
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:style w:type="paragraph" w:styleId="Clause"><w:name w:val="Clause"/><w:pPr><w:outlineLvl w:val="1"/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="2"/></w:pPr></w:style></w:styles>"#
    );
    let bytes = package(&[
        (
            "word/document.xml",
            format!(
                r#"<w:document xmlns:w="{W}"><w:body>{}{}<w:sectPr/></w:body></w:document>"#,
                styled("Named", "Heading1"),
                styled("Clause text", "Clause"),
            ),
        ),
        ("word/styles.xml", styles),
    ]);
    let src = SourceDocument::from_bytes("t.docx", InputFormat::Docx, bytes);
    let labels = |doc: &DoclingDocument| -> Vec<(String, Option<u8>)> {
        tree(doc)
            .items
            .iter()
            .filter_map(|it| match &it.kind {
                TreeKind::Text { label, level, .. } => Some((label.clone(), *level)),
                _ => None,
            })
            .collect()
    };
    let on = super::DocxBackend::default().convert(&src).unwrap();
    assert_eq!(md(&on), "#### Named\n\n### Clause text");
    assert_eq!(
        labels(&on),
        [
            ("section_header".to_string(), Some(3)),
            ("section_header".to_string(), Some(2))
        ]
    );
    let off = super::DocxBackend {
        use_outline_level_for_headings: false,
        ..Default::default()
    }
    .convert(&src)
    .unwrap();
    assert_eq!(md(&off), "#### Named\n\nClause text");
    assert_eq!(
        labels(&off),
        [
            ("section_header".to_string(), Some(3)),
            ("text".to_string(), None)
        ]
    );
}

// ---------------------------------------------------------------------------
// docling#4863, the iWork half: a Pages sub-list hangs from the item above.

/// `test_sub_list_belongs_to_the_item_above_it` — the JSON nests a deeper
/// level's group under the list item before it.
#[test]
fn pages_sub_list_belongs_to_the_item_above_it() {
    let bullet = r#"<sf:list-label-typeinfo><sf:text-label sf:type="bullet" sf:format="*"/></sf:list-label-typeinfo>"#;
    let xml = format!(
        r#"<?xml version="1.0"?><sl:document xmlns:sl="http://developer.apple.com/namespaces/sl" xmlns:sf="http://developer.apple.com/namespaces/sf">
          <sf:stylesheet><sf:liststyle sf:ident="ls-bullet"><sf:list-label-typeinfo sf:type="none"/>{bullet}{bullet}{bullet}</sf:liststyle></sf:stylesheet>
          <sf:text-storage><sf:text-body>
            <sf:p sf:list-style="ls-bullet" sf:list-level="1">outer one</sf:p>
            <sf:p sf:list-style="ls-bullet" sf:list-level="2">inner a</sf:p>
            <sf:p sf:list-style="ls-bullet" sf:list-level="3">inner a.1</sf:p>
            <sf:p sf:list-style="ls-bullet" sf:list-level="2">inner b</sf:p>
            <sf:p sf:list-style="ls-bullet" sf:list-level="1">outer two</sf:p>
          </sf:text-body></sf:text-storage></sl:document>"#
    );
    let src = SourceDocument::from_bytes(
        "nested.pages",
        InputFormat::Pages,
        package(&[("index.xml", xml)]),
    );
    let doc = super::IworkBackend.convert(&src).expect("converts");
    let json = doc.export_to_json_value();
    let texts = json["texts"].as_array().unwrap();
    let by_text = |t: &str| {
        texts
            .iter()
            .find(|x| x["text"] == t)
            .unwrap_or_else(|| panic!("no {t:?}"))
    };
    let parent = |t: &str| by_text(t)["parent"]["$ref"].as_str().unwrap().to_string();
    let self_ref = |t: &str| by_text(t)["self_ref"].as_str().unwrap().to_string();
    let group_parent = |g: &str| {
        let i: usize = g.rsplit('/').next().unwrap().parse().unwrap();
        json["groups"][i]["parent"]["$ref"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(parent("outer two"), parent("outer one"));
    let inner = parent("inner a");
    assert_eq!(group_parent(&inner), self_ref("outer one"));
    assert_eq!(parent("inner b"), inner);
    assert_eq!(group_parent(&parent("inner a.1")), self_ref("inner a"));
}
