//! PPTX (PowerPoint) backend.
//!
//! Ports docling's `MsPowerpointDocumentBackend`: each slide's shape tree is
//! walked in order, emitting titles, paragraphs, bullet/numbered lists, tables,
//! and pictures. Bullet detection follows docling's `_get_effective_list_marker`:
//! the paragraph's own `a:pPr` marker wins (`buNone`/`buChar`/`buAutoNum`/
//! `buBlip`), then the owning shape's `a:lstStyle` level properties for the
//! paragraph's level supply an inherited one (#406); with no marker anywhere,
//! body placeholders inherit a bullet from the master (so they default to a
//! list), an indented paragraph is a list item (docling's `level > 0`
//! fallback), and a plain text box paragraph stays a paragraph. The layout
//! placeholder's list style and the master's `p:txStyles` — the tail of
//! docling's chain — are not walked yet; the body-placeholder default stands in
//! for them.
//!
//! The walk builds two things at once. The flat [`Node`] stream is what
//! Markdown / DocLang / LaTeX render. The JSON export reads docling's item
//! tree instead ([`docling_core::tree::ItemTree`], filled here as
//! `MsPowerpointDocumentBackend._walk_linear` would): one `chapter` group per
//! slide, a `paragraph`/`title` text or a `list` group of `list_item`s per
//! text shape, tables with only their non-empty cells, pictures with the
//! file's dpi, charts as classified pictures with a caption, speaker notes
//! and `comment_section` groups on the `notes` layer — every item carrying
//! upstream's provenance verbatim (the shape's EMU box tagged `BOTTOMLEFT`,
//! a `charspan` over the item's text) and numbered in creation order. The
//! slides convert in parallel; each one's fragment is merged in slide order
//! with [`ItemTree::append`], which renumbers it as if built sequentially.

use std::collections::{HashMap, HashSet};

use docling_core::tree::{ItemTree, ListMeta, TreeKind, TreeProv};
use docling_core::{
    ContentLayer, DoclingDocument, Node, PictureImage, Table, TableCell, TableStructure,
};
use rayon::prelude::*;
use roxmltree::{Document, Node as XmlNode};

use crate::backend::ooxml::{
    content_type, image_dpi, is_metafile, picture_image, resolve, Package,
};
use crate::backend::xlsx_drawings;
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;

pub struct PptxBackend;

impl DeclarativeBackend for PptxBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let mut pkg = Package::open(&source.bytes)
            .ok_or_else(|| ConversionError::Parse("pptx: bad zip".into()))?;
        let mut doc = DoclingDocument::new(&source.name);

        let presentation = pkg
            .read("ppt/presentation.xml")
            .ok_or_else(|| ConversionError::Parse("pptx: no presentation.xml".into()))?;
        let slide_size = slide_size(&presentation);
        let content_types = pkg.read("[Content_Types].xml").unwrap_or_default();
        let rid_to_part: HashMap<String, String> = pkg
            .rels_for("ppt/presentation.xml")
            .iter()
            .map(|r| (r.id.clone(), resolve("ppt", &r.target)))
            .collect();
        let authors = comment_authors(&mut pkg);

        // Slides are independent, so they convert in parallel — each worker
        // clones the package (cheap: shared bytes + zip central directory)
        // and builds its fragment; the ordered collect keeps output identical
        // to the sequential walk.
        let slides: Vec<(usize, String)> = slide_rids(&presentation)
            .into_iter()
            .enumerate()
            .filter_map(|(ix, rid)| rid_to_part.get(&rid).map(|p| (ix, p.clone())))
            .collect();
        let frags: Vec<Option<SlideFrag>> = slides
            .par_iter()
            .map(|(ix, part)| {
                convert_slide(pkg.clone(), part, *ix, slide_size, &content_types, &authors)
            })
            .collect();
        let mut tree = ItemTree::default();
        for ((slide_ix, _), frag) in slides.into_iter().zip(frags) {
            let Some(SlideFrag {
                content,
                comments,
                tree: slide_tree,
            }) = frag
            else {
                continue;
            };
            tree.append(slide_tree);
            // docling records every slide as a page sized in EMU, which is
            // what its item provenance is expressed in (#402).
            doc.push(Node::PageInfo {
                page_no: slide_ix + 1,
                width: slide_size.0 as f32,
                height: slide_size.1 as f32,
            });
            // docling gives each slide a `chapter` group named `slide-{0-based}`
            // and hangs everything the slide holds off it (#402), so a note or
            // a caption belongs to its slide rather than to the document body.
            // Groups are transparent to Markdown and DocLang, so only the JSON
            // gains the structure.
            doc.push(Node::Group {
                label: "chapter".into(),
                name: Some(format!("slide-{slide_ix}")),
                layer: None,
                children: content,
            });
            // DocLang page break: docling's serializer places each slide
            // boundary's break *after* the following slide's content (every
            // slide beyond the first trails one — same artifact as XLSX
            // sheets, see the xlsx module docs).
            if slide_ix > 0 {
                doc.push(Node::PageBreak);
            }
            // Review comments (`p:cm`) carry no provenance, so they serialize
            // after the page break, matching docling's comment_section groups.
            doc.nodes.extend(comments);
        }
        // The JSON serializes docling's item tree (see the module docs); the
        // flat nodes above stay the source for every other serializer.
        doc.tree = Some(tree);
        Ok(doc)
    }
}

/// One converted slide: its flat content nodes (shapes + speaker notes), its
/// review-comment nodes, and its fragment of docling's item tree (the slide
/// group with everything under it, then the slide's `comment_section`
/// groups as body children — upstream's creation order within a slide).
struct SlideFrag {
    content: Vec<Node>,
    comments: Vec<Node>,
    tree: ItemTree,
}

/// What every shape handler on a slide reads.
struct SlideCtx<'a> {
    /// Relationship ids whose target is an image-typed part.
    valid_imgs: &'a HashSet<String>,
    /// The decodable ones, with their pixels.
    images: &'a HashMap<String, PictureImage>,
    /// The undecodable ones that are Windows metafiles: docling keeps those
    /// as payload-less pictures and drops any other undecodable image.
    metafiles: &'a HashSet<String>,
    /// Native chart parts by relationship id: kind, title, data grid.
    charts: &'a HashMap<String, (String, Option<String>, Table)>,
    slide_size: (i64, i64),
    phmap: &'a PhMap,
    /// 1-based slide number — docling's `page_no`.
    page_no: usize,
}

/// What every shape handler on a slide writes: the flat nodes and the tree
/// fragment, whose `slide` group every body item hangs off.
struct SlideOut {
    doc: DoclingDocument,
    tree: ItemTree,
    slide: usize,
}

/// Convert one slide part, or `None` when the part is absent or not parsable
/// XML (such a slide contributes nothing — not even a page break).
fn convert_slide(
    mut pkg: Package,
    part: &str,
    slide_ix: usize,
    slide_size: (i64, i64),
    content_types: &str,
    authors: &HashMap<String, (String, String)>,
) -> Option<SlideFrag> {
    // Placeholder geometry inherited from the slide's layout → master,
    // for shapes that carry no own `<a:xfrm>` (python-pptx resolves
    // `shape.left/top/...` up this chain).
    let phmap = slide_placeholders(&mut pkg, part);
    // Relationship ids whose target is a real, image-typed part — only
    // these become pictures (linked/missing/wrong-type blips are dropped,
    // matching python-pptx + PIL).
    let dir = part
        .rsplit_once('/')
        .map(|(d, _)| d)
        .unwrap_or("")
        .to_string();
    let mut valid_imgs: HashSet<String> = HashSet::new();
    let mut images: HashMap<String, PictureImage> = HashMap::new();
    let mut metafiles: HashSet<String> = HashSet::new();
    // Native charts (docling PR #3794): each chart part parsed up front,
    // keyed by its relationship id — kind classified from the plot area,
    // data from the embedded caches.
    let mut charts: HashMap<String, (String, Option<String>, docling_core::Table)> = HashMap::new();
    for r in pkg.rels_for(part) {
        let p = resolve(&dir, &r.target);
        if r.rel_type.ends_with("/chart") {
            if let Some(spec) = pkg.read(&p).as_deref().and_then(xlsx_drawings::parse_chart) {
                // python-pptx has no plot class for the 3-D chart variants
                // (`bar3DChart`, `line3DChart`, `pie3DChart`, …): upstream's
                // `chart.chart_type` fails there, the classification falls
                // back to `other_chart`, and since docling#3972 (2.120) the
                // data extraction failure is caught and the chart carries no
                // table. The Excel backend keeps its tagname map (3-D → the
                // 2-D family), so the override lives here, not in the parser.
                let three_d = spec.plot_tag.ends_with("3DChart");
                let kind = if three_d { "other_chart" } else { spec.kind };
                let table = if three_d {
                    Some(docling_core::Table::default())
                } else {
                    xlsx_drawings::chart_table_from_caches(&spec)
                };
                if let Some(table) = table {
                    charts.insert(r.id.clone(), (kind.to_string(), spec.title, table));
                }
            }
            continue;
        }
        if !content_type(content_types, &p)
            .map(|ct| ct.starts_with("image/"))
            .unwrap_or(false)
        {
            continue;
        }
        valid_imgs.insert(r.id.clone());
        // Decodable images carry their pixels for export; the rest still
        // emit a placeholder picture.
        if let Some(bytes) = pkg.read_bytes(&p) {
            let is_meta = is_metafile(&bytes);
            match picture_image(&p, bytes) {
                Some(img) => {
                    images.insert(r.id, img);
                }
                None if is_meta => {
                    metafiles.insert(r.id);
                }
                None => {}
            }
        }
    }

    let xml = pkg.read(part)?;
    let slide = Document::parse(&xml).ok()?;
    let page_no = slide_ix + 1;
    let ctx = SlideCtx {
        valid_imgs: &valid_imgs,
        images: &images,
        metafiles: &metafiles,
        charts: &charts,
        slide_size,
        phmap: &phmap,
        page_no,
    };
    let mut out = SlideOut {
        doc: DoclingDocument::new("slide"),
        tree: ItemTree::default(),
        slide: 0,
    };
    // docling's `slide-{0-based}` chapter group, created before the page.
    out.slide = out.tree.add(
        None,
        None,
        TreeKind::Group {
            label: "chapter".into(),
            name: format!("slide-{slide_ix}"),
        },
    );
    if let Some(tree) = descendant(slide.root_element(), "spTree") {
        for shape in shapes_by_position(tree, &phmap) {
            handle_shape(shape, &ctx, &mut out);
        }
    }
    // Speaker notes are slide content (docling gives them a zero-bbox
    // provenance on the slide's page), so they precede the page break.
    slide_notes(&mut pkg, part, &dir, page_no, &mut out);
    let mut comments = DoclingDocument::new("comments");
    slide_comments(
        &mut pkg,
        part,
        &dir,
        slide_ix,
        authors,
        &mut comments,
        &mut out.tree,
    );
    Some(SlideFrag {
        content: out.doc.nodes,
        comments: comments.nodes,
        tree: out.tree,
    })
}

/// Author id → (name, initials) from `ppt/commentAuthors.xml`.
fn comment_authors(pkg: &mut Package) -> HashMap<String, (String, String)> {
    let mut map = HashMap::new();
    let Some(xml) = pkg.read("ppt/commentAuthors.xml") else {
        return map;
    };
    let Ok(doc) = Document::parse(&xml) else {
        return map;
    };
    for a in doc.descendants().filter(|n| n.has_tag_name("cmAuthor")) {
        map.insert(
            a.attribute("id").unwrap_or("").to_string(),
            (
                a.attribute("name").unwrap_or("").to_string(),
                a.attribute("initials").unwrap_or("").to_string(),
            ),
        );
    }
    map
}

/// Emit a slide's speaker notes: python-pptx's `notes_text_frame.text` (the
/// body placeholder's paragraphs joined with newlines, soft breaks as `\v`),
/// stripped, as one notes-layer text with a zero-bbox location.
fn slide_notes(pkg: &mut Package, part: &str, dir: &str, page_no: usize, out: &mut SlideOut) {
    for r in pkg.rels_for(part) {
        if !r.rel_type.ends_with("/notesSlide") {
            continue;
        }
        let p = resolve(dir, &r.target);
        let Some(xml) = pkg.read(&p) else {
            continue;
        };
        let Ok(ndoc) = Document::parse(&xml) else {
            continue;
        };
        let body = ndoc.descendants().find(|n| {
            n.has_tag_name("sp")
                && n.descendants()
                    .any(|d| d.has_tag_name("ph") && d.attribute("type") == Some("body"))
        });
        let Some(tx) = body.and_then(|sp| descendant(sp, "txBody")) else {
            continue;
        };
        let text = tx
            .children()
            .filter(|n| n.has_tag_name("p"))
            .map(|p| {
                let mut s = String::new();
                for child in p.children().filter(XmlNode::is_element) {
                    match child.tag_name().name() {
                        "r" | "fld" => {
                            if let Some(t) = child.children().find(|n| n.has_tag_name("t")) {
                                s.push_str(t.text().unwrap_or(""));
                            }
                        }
                        "br" => s.push('\u{b}'),
                        _ => {}
                    }
                }
                s
            })
            .collect::<Vec<_>>()
            .join("\n");
        let text = text.trim();
        if !text.is_empty() {
            out.doc.push(Node::Furniture {
                layer: ContentLayer::Notes,
                inner: Box::new(Node::Located {
                    location: [0, 0, 0, 0],
                    inner: Box::new(Node::Paragraph {
                        text: text.to_string(),
                    }),
                }),
            });
            // docling: a `text` item on the notes layer under the slide, with
            // a zero `BoundingBox()` — top-left origin, unlike the shapes.
            out.tree.add_with_prov(
                Some(out.slide),
                Some(ContentLayer::Notes),
                text_kind("text", text),
                TreeProv {
                    page_no,
                    bbox: [0.0; 4],
                    bottom_left: false,
                    charspan: [0, text.chars().count()],
                },
            );
        }
    }
}

/// A plain text item of the tree: docling's `add_text(label, text)`.
fn text_kind(label: &str, text: &str) -> TreeKind {
    TreeKind::Text {
        label: label.into(),
        text: text.into(),
        orig: None,
        formatting: None,
        hyperlink: None,
        level: None,
        list: None,
    }
}

/// Emit a slide's review comments as notes-layer texts, docling's format:
/// `[author: Name (IN), time: dt]: text` (either metadata part may be absent;
/// `dt` is the raw attribute string). In the tree each one is a
/// `comment_section` group named `comment-slide{page}-{idx}` (the comment's
/// `idx`, else the 0-based slide index) under the body, holding the note.
fn slide_comments(
    pkg: &mut Package,
    part: &str,
    dir: &str,
    slide_ix: usize,
    authors: &HashMap<String, (String, String)>,
    doc: &mut DoclingDocument,
    tree: &mut ItemTree,
) {
    for r in pkg.rels_for(part) {
        if !r.rel_type.ends_with("/comments") {
            continue;
        }
        let p = resolve(dir, &r.target);
        let Some(xml) = pkg.read(&p) else {
            continue;
        };
        let Ok(cdoc) = Document::parse(&xml) else {
            continue;
        };
        for cm in cdoc.descendants().filter(|n| n.has_tag_name("cm")) {
            let text = cm
                .children()
                .find(|n| n.has_tag_name("text"))
                .and_then(|t| t.text())
                .unwrap_or("")
                .trim();
            if text.is_empty() {
                continue;
            }
            let mut meta = Vec::new();
            if let Some((name, initials)) = authors.get(cm.attribute("authorId").unwrap_or("")) {
                if !name.is_empty() {
                    let mut a = format!("author: {name}");
                    if !initials.is_empty() {
                        a.push_str(&format!(" ({initials})"));
                    }
                    meta.push(a);
                }
            }
            if let Some(dt) = cm.attribute("dt").filter(|d| !d.is_empty()) {
                meta.push(format!("time: {dt}"));
            }
            let full = if meta.is_empty() {
                text.to_string()
            } else {
                format!("[{}]: {}", meta.join(", "), text)
            };
            doc.push(Node::Furniture {
                layer: ContentLayer::Notes,
                inner: Box::new(Node::Paragraph { text: full.clone() }),
            });
            let idx = cm
                .attribute("idx")
                .unwrap_or(&slide_ix.to_string())
                .to_string();
            let group = tree.add(
                None,
                Some(ContentLayer::Notes),
                TreeKind::Group {
                    label: "comment_section".into(),
                    name: format!("comment-slide{}-{idx}", slide_ix + 1),
                },
            );
            tree.add(
                Some(group),
                Some(ContentLayer::Notes),
                text_kind("text", &full),
            );
        }
    }
}

/// Ordered slide relationship ids from `<p:sldIdLst>`.
fn slide_rids(presentation: &str) -> Vec<String> {
    let Ok(doc) = Document::parse(presentation) else {
        return Vec::new();
    };
    doc.descendants()
        .filter(|n| n.has_tag_name("sldId"))
        .filter_map(|n| {
            n.attributes()
                .find(|a| a.name() == "id" && a.namespace().is_some())
                .map(|a| a.value().to_string())
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
/// Order a shape collection in visual reading order (docling#3393's
/// `_iter_shapes_by_position`): resolve each shape's top/left (its own
/// `<a:xfrm>`, else the inherited placeholder geometry — python-pptx's
/// `shape.top`), sort by top (stable, so ties keep z-order), band shapes whose
/// top sits within 0.05" (45720 EMU) of the *previous* shape's top into one
/// row — adjacency is chained, so a contiguous band can span several
/// tolerance steps — and sort each row left-to-right. Shapes with no
/// resolvable geometry sort last in their original order (upstream's
/// `fallback_position`).
fn shapes_by_position<'a>(tree: XmlNode<'a, 'a>, phmap: &PhMap) -> Vec<XmlNode<'a, 'a>> {
    const ROW_TOLERANCE_EMU: i64 = 45720; // 0.05 inch

    struct Info<'a> {
        index: usize,
        node: XmlNode<'a, 'a>,
        top: i64,
        left: i64,
    }
    let mut infos: Vec<Info<'a>> = tree
        .children()
        .filter(|n| matches!(n.tag_name().name(), "sp" | "grpSp" | "pic" | "graphicFrame"))
        .enumerate()
        .map(|(index, node)| {
            let geom = xfrm_geom(node).or_else(|| inherited_geom(node, phmap));
            let (left, top) = geom.map_or((i64::MAX, i64::MAX), |g| (g[0], g[1]));
            Info {
                index,
                node,
                top,
                left,
            }
        })
        .collect();
    infos.sort_by_key(|s| (s.top, s.index));

    let mut out = Vec::with_capacity(infos.len());
    let mut row: Vec<Info<'a>> = Vec::new();
    let mut prev_top: Option<i64> = None;
    fn flush<'a>(row: &mut Vec<Info<'a>>, out: &mut Vec<XmlNode<'a, 'a>>) {
        row.sort_by_key(|s| (s.left, s.index));
        out.extend(row.drain(..).map(|s| s.node));
    }
    for info in infos {
        if prev_top.is_some_and(|p| info.top - p > ROW_TOLERANCE_EMU) {
            flush(&mut row, &mut out);
        }
        prev_top = Some(info.top);
        row.push(info);
    }
    flush(&mut row, &mut out);
    out
}

fn handle_shape(shape: XmlNode, ctx: &SlideCtx, out: &mut SlideOut) {
    let location = shape_location(shape, ctx.slide_size, ctx.phmap);
    match shape.tag_name().name() {
        "grpSp" => {
            // Group children re-sort in visual order too (docling#3393); their
            // coordinates share the group's child space, so relative order is
            // well-defined. Groups carry no placeholder geometry — the phmap
            // lookup just never fires for them.
            for child in shapes_by_position(shape, ctx.phmap) {
                handle_shape(child, ctx, out);
            }
        }
        "graphicFrame" => {
            if let Some(tbl) = descendant(shape, "tbl") {
                if let Some(table) = parse_table(tbl) {
                    // docling keeps only the cells with text (`_handle_tables`)
                    // and skips a table with none at all.
                    let cells = tree_table_cells(tbl);
                    if !cells.is_empty() {
                        out.tree.add_with_prov(
                            Some(out.slide),
                            None,
                            TreeKind::Table {
                                table: Table {
                                    rows: table.rows.clone(),
                                    cells: Some(cells),
                                    ..Table::default()
                                },
                                rich_cells: Vec::new(),
                                captions: Vec::new(),
                            },
                            shape_prov(shape, ctx, 0),
                        );
                    }
                    push_located(&mut out.doc, location, Node::Table(table));
                }
            } else if let Some((kind, title, table)) = descendant(shape, "chart")
                .and_then(|c| c.attributes().find(|a| a.name() == "id"))
                .and_then(|a| ctx.charts.get(a.value()))
            {
                // A native chart frame (docling PR #3794): classified kind +
                // the cached data grid, chart title as the caption. docling
                // adds the caption to the slide first — with the chart's box
                // and a charspan over the title — then the picture.
                let caption = title.as_deref().filter(|t| !t.is_empty()).map(|t| {
                    out.tree.add_with_prov(
                        Some(out.slide),
                        None,
                        text_kind("caption", t),
                        shape_prov(shape, ctx, t.chars().count()),
                    )
                });
                out.tree.add_with_prov(
                    Some(out.slide),
                    None,
                    TreeKind::Picture {
                        captions: caption.into_iter().collect(),
                        image: None,
                        classification: Some(kind.clone()),
                        confidence: None,
                        chart: Some(table.clone()),
                        dpi: None,
                    },
                    shape_prov(shape, ctx, 0),
                );
                out.doc.push(Node::Chart {
                    kind: kind.clone(),
                    table: table.clone(),
                    caption: title.clone(),
                    location: Some(location),
                });
            }
        }
        "pic" => {
            // Emit only loadable embedded images (an `r:embed` into an image part).
            let embedded = descendant(shape, "blip").and_then(|b| {
                b.attributes()
                    .find(|a| a.name() == "embed")
                    .map(|a| a.value().to_string())
            });
            if let Some(rid) = embedded.filter(|rid| ctx.valid_imgs.contains(rid)) {
                let image = ctx.images.get(&rid);
                // docling: PIL decodes it → a picture with the image (at the
                // file's dpi); a metafile PIL cannot → a payload-less picture;
                // anything else undecodable → no picture at all.
                if image.is_some() || ctx.metafiles.contains(&rid) {
                    out.tree.add_with_prov(
                        Some(out.slide),
                        None,
                        TreeKind::Picture {
                            captions: Vec::new(),
                            image: image.cloned(),
                            classification: None,
                            confidence: None,
                            chart: None,
                            dpi: image.map(|img| pptx_dpi(&img.data)),
                        },
                        shape_prov(shape, ctx, 0),
                    );
                }
                push_located(
                    &mut out.doc,
                    location,
                    Node::Picture {
                        caption: None,
                        caption_href: None,
                        image: image.cloned(),
                        classification: None,
                        caption_parent: Default::default(),
                    },
                );
            }
        }
        "sp" => handle_text_shape(shape, location, ctx, out),
        _ => {}
    }
}

/// docling's `_generate_prov`: the shape's box in EMU — `shape.left/top/
/// width/height` (its own `<a:xfrm>`, else the placeholder geometry it
/// inherits), the whole slide when none resolves — handed to
/// `BoundingBox.from_tuple((l, t, l + w, t + h), origin=BOTTOMLEFT)`, which
/// reads a bottom-left tuple as `(l, b, r, t)`: the JSON's `b` is the shape's
/// top EMU and `t` its bottom, tagged `BOTTOMLEFT`. A quirk, kept verbatim.
/// `charspan` covers `char_len` characters.
fn shape_prov(shape: XmlNode, ctx: &SlideCtx, char_len: usize) -> TreeProv {
    let (w, h) = ctx.slide_size;
    let [x, y, cx, cy] = xfrm_geom(shape)
        .or_else(|| inherited_geom(shape, ctx.phmap))
        .unwrap_or([0, 0, w, h]);
    let (mut l, mut b, mut r, mut t) = (x, y, x + cx, y + cy);
    // `from_tuple`'s normalization: `l <= r`, and for a bottom-left box `b <= t`.
    if r < l {
        std::mem::swap(&mut l, &mut r);
    }
    if b > t {
        std::mem::swap(&mut b, &mut t);
    }
    TreeProv {
        page_no: ctx.page_no,
        bbox: [l as f64, t as f64, r as f64, b as f64],
        bottom_left: true,
        charspan: [0, char_len],
    }
}

/// python-pptx's `Image.dpi`: PIL's horizontal dpi rounded (half to even,
/// Python's `round`), 72 when the file records none or the value falls
/// outside 1–2048.
fn pptx_dpi(data: &[u8]) -> u32 {
    match image_dpi(data).map(f64::round_ties_even) {
        Some(d) if (1.0..=2048.0).contains(&d) => d as u32,
        _ => 72,
    }
}

/// The `TableCell`s docling's `_handle_tables` records for a table: one per
/// `<a:tc>` (merge continuations included — python-pptx's `row.cells`) whose
/// stripped text is non-empty, spanning its `rowSpan` × `gridSpan`, every
/// first-row cell a column header.
fn tree_table_cells(tbl: XmlNode) -> Vec<TableCell> {
    let mut cells = Vec::new();
    for (ri, row) in tbl.children().filter(|n| n.has_tag_name("tr")).enumerate() {
        for (ci, tc) in row.children().filter(|n| n.has_tag_name("tc")).enumerate() {
            let text = cell_text(tc);
            if text.is_empty() {
                continue;
            }
            let span = |name: &str| -> usize {
                tc.attribute(name).and_then(|s| s.parse().ok()).unwrap_or(1)
            };
            cells.push(TableCell {
                text,
                bbox: None,
                start_row: ri,
                start_col: ci,
                row_span: span("rowSpan"),
                col_span: span("gridSpan"),
                column_header: ri == 0,
                row_header: false,
                row_section: false,
            });
        }
    }
    cells
}

/// Slide size (EMU) from `<p:sldSz cx cy>`, defaulting to the 4:3 standard.
fn slide_size(presentation: &str) -> (i64, i64) {
    Document::parse(presentation)
        .ok()
        .and_then(|d| {
            let sz = d.descendants().find(|n| n.has_tag_name("sldSz"))?;
            Some((
                sz.attribute("cx")?.parse().ok()?,
                sz.attribute("cy")?.parse().ok()?,
            ))
        })
        .unwrap_or((9144000, 6858000))
}

/// docling's `_generate_prov`: the shape's bbox normalized to DocLang's 0–511
/// grid. The bbox is bottom-left origin (so y is flipped). The shape's geometry
/// is its own `<a:xfrm>` if present, else the placeholder box it inherits from
/// the slide layout/master (python-pptx `shape.left/top/...`). A shape with no
/// resolvable geometry — or `left == 0` (docling's `if shape.left:` truthiness)
/// — takes the whole slide.
fn shape_location(shape: XmlNode, (w, h): (i64, i64), phmap: &PhMap) -> [u16; 4] {
    let geom = xfrm_geom(shape).or_else(|| inherited_geom(shape, phmap));
    // A shape at x = 0 EMU keeps its own box (docling#3990, 2.120): the old
    // `if shape.left:` truthiness test treated a zero offset like a missing
    // one and fell back to the whole slide.
    let (left, top, cw, ch) = match geom {
        Some([x, y, cx, cy]) => (x, y, cx, cy),
        None => (0, 0, w, h),
    };
    let n = |v: i64, dim: i64| -> u16 {
        if dim == 0 {
            return 0;
        }
        ((512.0 * v as f64 / dim as f64).round() as i64).clamp(0, 511) as u16
    };
    [
        n(left, w),
        n(h - (top + ch), h),
        n(left + cw, w),
        n(h - top, h),
    ]
}

/// A shape/placeholder's own transform `[x, y, cx, cy]` in EMU, from its
/// `<a:xfrm>` (`<p:xfrm>` for a graphic frame — `descendant` matches either).
fn xfrm_geom(node: XmlNode) -> Option<[i64; 4]> {
    let x = descendant(node, "xfrm")?;
    let off = x.children().find(|n| n.has_tag_name("off"))?;
    let ext = x.children().find(|n| n.has_tag_name("ext"))?;
    Some([
        off.attribute("x")?.parse().ok()?,
        off.attribute("y")?.parse().ok()?,
        ext.attribute("cx")?.parse().ok()?,
        ext.attribute("cy")?.parse().ok()?,
    ])
}

/// The geometry a placeholder shape inherits from its layout/master: match its
/// `<p:ph>` by `idx` first, then by `type` (python-pptx's inheritance keys).
fn inherited_geom(shape: XmlNode, phmap: &PhMap) -> Option<[i64; 4]> {
    let ph = descendant(shape, "ph")?;
    if let Some(idx) = ph.attribute("idx") {
        if let Some(g) = phmap.by_idx.get(idx) {
            return Some(*g);
        }
    }
    if let Some(t) = ph.attribute("type") {
        if let Some(g) = phmap.by_type.get(t) {
            return Some(*g);
        }
    }
    None
}

/// Placeholder geometries a slide can inherit, keyed by `<p:ph>` `idx` and
/// `type`. The layout is consulted before the master (layout wins).
#[derive(Default)]
struct PhMap {
    by_idx: HashMap<String, [i64; 4]>,
    by_type: HashMap<String, [i64; 4]>,
}

/// Build the [`PhMap`] for a slide: its layout part (via the slide's `.rels`)
/// then that layout's master (via the layout's `.rels`). Placeholders already
/// seen (layout) are not overwritten by the master.
fn slide_placeholders(pkg: &mut Package, slide_part: &str) -> PhMap {
    let mut map = PhMap::default();
    let slide_dir = slide_part.rsplit_once('/').map_or("", |(d, _)| d);
    let Some(layout_part) = rel_target(pkg, slide_part, slide_dir, "/slideLayout") else {
        return map;
    };
    if let Some(xml) = pkg.read(&layout_part) {
        collect_placeholders(&xml, &mut map);
    }
    let layout_dir = layout_part.rsplit_once('/').map_or("", |(d, _)| d);
    if let Some(master_part) = rel_target(pkg, &layout_part, layout_dir, "/slideMaster") {
        if let Some(xml) = pkg.read(&master_part) {
            collect_placeholders(&xml, &mut map);
        }
    }
    map
}

/// Resolve the first relationship of `part` whose type ends with `suffix` to a
/// package path (against `base_dir`).
fn rel_target(pkg: &mut Package, part: &str, base_dir: &str, suffix: &str) -> Option<String> {
    pkg.rels_for(part)
        .iter()
        .find(|r| r.rel_type.ends_with(suffix))
        .map(|r| resolve(base_dir, &r.target))
}

/// Record every placeholder's own `<a:xfrm>` geometry from a layout/master part,
/// keyed by its `<p:ph>` `idx` and `type`. `or_insert` keeps the earlier source
/// (layout before master).
fn collect_placeholders(xml: &str, map: &mut PhMap) {
    let Ok(doc) = Document::parse(xml) else {
        return;
    };
    let Some(tree) = descendant(doc.root_element(), "spTree") else {
        return;
    };
    for sp in tree.children().filter(|n| n.has_tag_name("sp")) {
        let Some(ph) = descendant(sp, "ph") else {
            continue;
        };
        let Some(geom) = xfrm_geom(sp) else {
            continue;
        };
        if let Some(idx) = ph.attribute("idx") {
            map.by_idx.entry(idx.to_string()).or_insert(geom);
        }
        if let Some(t) = ph.attribute("type") {
            map.by_type.entry(t.to_string()).or_insert(geom);
        }
    }
}

/// Wrap `node` in a [`Node::Located`] carrying the shape's provenance.
fn push_located(doc: &mut DoclingDocument, location: [u16; 4], node: Node) {
    doc.push(Node::Located {
        location,
        inner: Box::new(node),
    });
}

#[derive(Clone, Copy, PartialEq)]
enum Placeholder {
    Title,
    Subtitle,
    Body,
    TextBox,
}

fn placeholder_kind(sp: XmlNode) -> Placeholder {
    match descendant(sp, "ph") {
        None => Placeholder::TextBox,
        Some(ph) => match ph.attribute("type") {
            Some("title") | Some("ctrTitle") => Placeholder::Title,
            Some("subTitle") => Placeholder::Subtitle,
            _ => Placeholder::Body,
        },
    }
}

fn handle_text_shape(sp: XmlNode, location: [u16; 4], ctx: &SlideCtx, out: &mut SlideOut) {
    let Some(tx_body) = descendant(sp, "txBody") else {
        return;
    };
    // docling skips a shape whose whole text is blank.
    let kind = placeholder_kind(sp);
    let paragraphs: Vec<XmlNode> = tx_body.children().filter(|n| n.has_tag_name("p")).collect();
    if paragraphs
        .iter()
        .all(|p| paragraph_text(*p).trim().is_empty())
    {
        return;
    }

    let mut in_list = false;
    let mut number = 0u64;
    // The open `list` group of the tree, while a run of list paragraphs lasts.
    let mut list_group: Option<usize> = None;
    for para in paragraphs {
        let text = paragraph_text(para);
        // docling's `charspan` is over the paragraph's own text, `len()` in
        // code points.
        let prov = shape_prov(sp, ctx, text.chars().count());
        match list_kind(para, Some(tx_body), kind) {
            Some(numbered) => {
                // docling opens one ListGroup per run of list paragraphs in a
                // shape (`new_list`, reset by a non-list paragraph): the first
                // item of the run starts the list, whatever marker kinds follow.
                let first_in_list = !in_list;
                if !in_list {
                    in_list = true;
                    number = 0;
                }
                let n = if numbered {
                    number += 1;
                    number
                } else {
                    0
                };
                // docling passes numbered items an `"N."` enumeration marker
                // and bulleted ones an empty one.
                let marker = numbered.then(|| format!("{n}."));
                let group = *list_group.get_or_insert_with(|| {
                    out.tree.add(
                        Some(out.slide),
                        None,
                        TreeKind::Group {
                            label: "list".into(),
                            name: "list".into(),
                        },
                    )
                });
                out.tree.add_with_prov(
                    Some(group),
                    None,
                    TreeKind::Text {
                        label: "list_item".into(),
                        text: text.clone(),
                        orig: None,
                        formatting: None,
                        hyperlink: None,
                        level: None,
                        list: Some(ListMeta {
                            enumerated: numbered,
                            marker: marker.clone().unwrap_or_default(),
                        }),
                    },
                    prov,
                );
                // Each item carries its shape's `<location>` (all items of a
                // body placeholder share the one box); the location rides on the
                // item itself so consecutive items still group into one `<list>`.
                out.doc.push(Node::ListItem {
                    ordered: numbered,
                    number: n,
                    first_in_list,
                    text,
                    level: 0,
                    marker,
                    location: Some(location),
                    dclx: None,
                    href: None,
                    layer: None,
                });
            }
            None => {
                in_list = false;
                list_group = None;
                // docling labels a title placeholder's text `title` and any
                // other non-list text `paragraph` (a `text` in Markdown terms).
                let label = match kind {
                    Placeholder::Title => "title",
                    _ => "paragraph",
                };
                out.tree
                    .add_with_prov(Some(out.slide), None, text_kind(label, &text), prov);
                match kind {
                    Placeholder::Title => {
                        push_located(&mut out.doc, location, Node::Heading { level: 1, text })
                    }
                    // A subtitle placeholder is a paragraph on purpose: docling
                    // settled it in docling#3785 and, after briefly moving it
                    // to SECTION_HEADER behind an option, reverted to that in
                    // docling#4190 (which also deleted the dead `_handle_title`
                    // that would have labelled it). Not a bug to fix.
                    _ => push_located(&mut out.doc, location, Node::Paragraph { text }),
                }
            }
        }
    }
}

/// The marker a properties element (`a:pPr` or `a:lvl{N}pPr`) declares, if it
/// declares one at all — docling's `_parse_bullet_from_paragraph_properties`.
/// `Some(None)` is an explicit `buNone`: "this is deliberately not a list".
fn declared_marker(p_pr: XmlNode) -> Option<Option<bool>> {
    if p_pr.children().any(|n| n.has_tag_name("buNone")) {
        return Some(None);
    }
    if p_pr.children().any(|n| n.has_tag_name("buAutoNum")) {
        return Some(Some(true));
    }
    if p_pr
        .children()
        .any(|n| n.has_tag_name("buChar") || n.has_tag_name("buBlip"))
    {
        return Some(Some(false));
    }
    None
}

/// A paragraph's outline level (`a:pPr/@lvl`, 0 when absent), which selects the
/// `a:lvl{lvl+1}pPr` that supplies an inherited marker.
fn paragraph_level(para: XmlNode) -> usize {
    para.children()
        .find(|n| n.has_tag_name("pPr"))
        .and_then(|pr| pr.attribute("lvl"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Whether a paragraph is a list item: `Some(true)` numbered, `Some(false)`
/// bulleted, `None` not a list.
///
/// docling's `_get_effective_list_marker` order: the paragraph's own `a:pPr`
/// first, then — and this is what a styled deck relies on — the *owning shape's*
/// list style, `a:txBody/a:lstStyle/a:lvl{lvl+1}pPr` selected by the paragraph's
/// level (#406). The level only picks which level properties supply the marker;
/// the items are not nested.
fn list_kind(para: XmlNode, tx_body: Option<XmlNode>, placeholder: Placeholder) -> Option<bool> {
    let lvl = paragraph_level(para);
    if let Some(p_pr) = para.children().find(|n| n.has_tag_name("pPr")) {
        if let Some(kind) = declared_marker(p_pr) {
            return kind;
        }
    }
    if let Some(lvl_pr) = tx_body
        .and_then(|b| b.children().find(|n| n.has_tag_name("lstStyle")))
        .and_then(|st| {
            st.children()
                .find(|n| n.has_tag_name(format!("lvl{}pPr", lvl + 1).as_str()))
        })
    {
        if let Some(kind) = declared_marker(lvl_pr) {
            return kind;
        }
    }
    // No marker anywhere in the shape: body placeholders inherit a bullet from
    // the master (the layout/master `lstStyle` chain in docling), and any
    // indented paragraph is a list item even without one — docling's
    // `paragraph.level > 0` fallback.
    match placeholder {
        Placeholder::Body => Some(false),
        _ => (lvl > 0).then_some(false),
    }
}

/// Concatenate a paragraph's run text; line breaks (`<a:br>`) become spaces.
fn paragraph_text(para: XmlNode) -> String {
    let mut out = String::new();
    for child in para.children().filter(XmlNode::is_element) {
        match child.tag_name().name() {
            "r" | "fld" => {
                if let Some(t) = child.children().find(|n| n.has_tag_name("t")) {
                    out.push_str(t.text().unwrap_or(""));
                }
            }
            "br" => out.push(' '),
            _ => {}
        }
    }
    out
}

fn parse_table(tbl: XmlNode) -> Option<Table> {
    let rows: Vec<XmlNode> = tbl.children().filter(|n| n.has_tag_name("tr")).collect();
    let num_cols = rows
        .iter()
        .map(|r| r.children().filter(|n| n.has_tag_name("tc")).count())
        .max()
        .unwrap_or(0);
    if rows.is_empty() || num_cols == 0 {
        return None;
    }
    // `<a:tblPr firstRow="1">` marks the first row as a header band (→ `<ched/>`).
    let first_row_header = descendant(tbl, "tblPr")
        .and_then(|p| p.attribute("firstRow"))
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);

    // Every grid position has its own `<a:tc>` (merge continuations included), so
    // the column index maps directly. A `hMerge` continuation is a horizontal
    // span (`<lcel/>`), a `vMerge` continuation a vertical span (`<ucel/>`); the
    // structure overlay carries those for DocLang. The `rows` text grid keeps
    // docling's Markdown/JSON behaviour: the origin cell's text is replicated
    // across its whole `rowSpan × gridSpan` region.
    let mut grid = vec![vec![String::new(); num_cols]; rows.len()];
    let mut col_continuation = vec![vec![false; num_cols]; rows.len()];
    let mut row_continuation = vec![vec![false; num_cols]; rows.len()];
    for (ri, row) in rows.iter().enumerate() {
        let cells: Vec<XmlNode> = row.children().filter(|n| n.has_tag_name("tc")).collect();
        for (ci, tc) in cells.iter().enumerate().take(num_cols) {
            let h = tc.attribute("hMerge").is_some();
            let v = tc.attribute("vMerge").is_some();
            col_continuation[ri][ci] = h;
            row_continuation[ri][ci] = v;
            // Continuation cells carry no text of their own (DocLang emits only
            // the token); the origin below fills their grid text for Markdown.
            if h || v {
                continue;
            }
            let text = cell_text(*tc);
            let span = |name: &str| -> usize {
                tc.attribute(name).and_then(|s| s.parse().ok()).unwrap_or(1)
            };
            let row_end = (ri + span("rowSpan")).min(rows.len());
            let col_end = (ci + span("gridSpan")).min(num_cols);
            for grow in grid.iter_mut().take(row_end).skip(ri) {
                for cell in grow.iter_mut().take(col_end).skip(ci) {
                    *cell = text.clone();
                }
            }
        }
    }
    let header_row = (0..rows.len())
        .map(|ri| first_row_header && ri == 0)
        .collect();
    Some(Table {
        rows: grid,
        location: None,
        structure: Some(TableStructure {
            header_row,
            col_continuation,
            row_continuation,
            row_header: Vec::new(),
            col_header: Vec::new(),
        }),
        cell_blocks: None,
        cells: None,
        caption: None,
        caption_parent: Default::default(),
    })
}

/// A table cell's text: its paragraphs joined with newlines, then trimmed
/// (matching python-pptx `cell.text.strip()`; the serializer turns `\n` into a
/// space).
fn cell_text(tc: XmlNode) -> String {
    let Some(tx_body) = descendant(tc, "txBody") else {
        return String::new();
    };
    tx_body
        .children()
        .filter(|n| n.has_tag_name("p"))
        .map(|p| paragraph_text(p))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// First descendant element with the given local tag name.
fn descendant<'a, 'input>(node: XmlNode<'a, 'input>, name: &str) -> Option<XmlNode<'a, 'input>> {
    node.descendants().find(|n| n.has_tag_name(name))
}
#[cfg(test)]
mod chart_tests {
    use super::*;
    use crate::backend::DeclarativeBackend;
    use crate::{InputFormat, SourceDocument};

    /// docling PR #3794: a native chart frame becomes a classified chart with
    /// its cached data grid and the chart title as the caption.
    #[test]
    fn native_chart_yields_classified_data_grid() {
        let path = format!(
            "{}/../../tests/data/pptx/sources/pptx_chart.pptx",
            env!("CARGO_MANIFEST_DIR")
        );
        let bytes = std::fs::read(&path).expect("fixture exists");
        let src = SourceDocument::from_bytes("c.pptx", InputFormat::Pptx, bytes);
        let doc = PptxBackend.convert(&src).expect("converts");
        let chart = doc
            .nodes
            .iter()
            // Slide content hangs off the slide's own group (#402).
            .flat_map(|n| match n {
                Node::Group { children, .. } => children.as_slice(),
                other => std::slice::from_ref(other),
            })
            .find_map(|n| match n {
                Node::Chart {
                    kind,
                    table,
                    caption,
                    ..
                } => Some((kind.clone(), table.clone(), caption.clone())),
                _ => None,
            })
            .expect("a chart node");
        assert_eq!(chart.0, "bar_chart");
        assert_eq!(
            chart.2.as_deref(),
            Some("Wild Duck Observations by Year"),
            "chart title as caption"
        );
        assert_eq!(chart.1.rows[0][1], "Freshwater Ducks");
        assert_eq!(chart.1.rows[1], vec!["2019", "120", "80"]);
    }
}

#[cfg(test)]
mod list_marker_tests {
    use super::{list_kind, Placeholder};

    /// #406: a paragraph with no bullet properties of its own takes its marker
    /// from the owning shape's `a:lstStyle`, selected by the paragraph's level —
    /// docling's `_get_effective_list_marker` step 2. The level only picks the
    /// `a:lvl{N}pPr`; it does not nest the item.
    #[test]
    fn a_shape_list_style_supplies_the_inherited_marker() {
        let xml = r#"<p:sp xmlns:p="p" xmlns:a="a"><p:txBody>
            <a:lstStyle>
              <a:lvl1pPr><a:buChar char="■"/></a:lvl1pPr>
              <a:lvl2pPr><a:buChar char="–"/></a:lvl2pPr>
              <a:lvl3pPr><a:buAutoNum type="arabicPeriod"/></a:lvl3pPr>
              <a:lvl4pPr><a:buNone/></a:lvl4pPr>
            </a:lstStyle>
            <a:p><a:r><a:t>level 1 bullet</a:t></a:r></a:p>
            <a:p><a:pPr lvl="1"/><a:r><a:t>level 2 bullet</a:t></a:r></a:p>
            <a:p><a:pPr lvl="2"/><a:r><a:t>level 3 numbered</a:t></a:r></a:p>
            <a:p><a:pPr lvl="3"/><a:r><a:t>level 4: style says no bullet</a:t></a:r></a:p>
            <a:p><a:pPr lvl="5"/><a:r><a:t>level 6: no style at all</a:t></a:r></a:p>
            <a:p><a:pPr lvl="2"><a:buNone/></a:pPr><a:r><a:t>own buNone wins</a:t></a:r></a:p>
        </p:txBody></p:sp>"#;
        let dom = roxmltree::Document::parse(xml).unwrap();
        let body = dom
            .descendants()
            .find(|n| n.has_tag_name("txBody"))
            .unwrap();
        let kinds: Vec<Option<bool>> = body
            .children()
            .filter(|n| n.has_tag_name("p"))
            .map(|p| list_kind(p, Some(body), Placeholder::TextBox))
            .collect();
        assert_eq!(
            kinds,
            vec![
                Some(false), // lvl1pPr buChar
                Some(false), // lvl2pPr buChar
                Some(true),  // lvl3pPr buAutoNum
                None,        // lvl4pPr buNone: deliberately not a list
                Some(false), // no marker anywhere, but indented: docling's level > 0 fallback
                None,        // the paragraph's own buNone beats the style's buAutoNum
            ]
        );
    }

    /// Without a shape list style the old rule stands: a body placeholder
    /// inherits a bullet from the master, a plain text box is a paragraph.
    #[test]
    fn without_a_list_style_the_placeholder_default_stands() {
        let dom = roxmltree::Document::parse(
            r#"<p:sp xmlns:p="p" xmlns:a="a"><p:txBody><a:lstStyle/><a:p><a:r><a:t>x</a:t></a:r></a:p></p:txBody></p:sp>"#,
        )
        .unwrap();
        let body = dom
            .descendants()
            .find(|n| n.has_tag_name("txBody"))
            .unwrap();
        let para = body.children().find(|n| n.has_tag_name("p")).unwrap();
        assert_eq!(list_kind(para, Some(body), Placeholder::Body), Some(false));
        assert_eq!(list_kind(para, Some(body), Placeholder::TextBox), None);
    }
}

#[cfg(test)]
mod json_tree_tests {
    use super::*;
    use crate::backend::ooxml::image_dpi_tests::png;
    use crate::backend::DeclarativeBackend;
    use crate::{InputFormat, SourceDocument};
    use serde_json::Value;

    /// Drop what a conformance comparison never looks at: the file's
    /// `origin` and schema `version`, and an image's bytes (PIL re-encodes
    /// them; ours are the embedded file's).
    fn normalize(v: &mut Value) {
        match v {
            Value::Object(m) => {
                m.remove("origin");
                m.remove("version");
                if let Some(uri) = m.get_mut("uri") {
                    *uri = Value::Null;
                }
                m.values_mut().for_each(normalize);
            }
            Value::Array(a) => a.iter_mut().for_each(normalize),
            _ => {}
        }
    }

    /// The JSON is docling's item tree: on every mirrored fixture it is
    /// structurally identical to upstream's groundtruth (docling 2.129) —
    /// slide groups, `paragraph`/`title`/`list_item` labels and markers,
    /// list groups, the non-empty table cells, pictures with the file's
    /// dpi, chart captions, notes, `comment_section` groups, and every
    /// item's raw-EMU provenance with its per-item `charspan`.
    #[test]
    fn json_is_structurally_identical_to_docling_groundtruth() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/pptx");
        let mut compared = 0;
        for entry in std::fs::read_dir(root.join("sources")).expect("pptx corpus") {
            let path = entry.expect("entry").path();
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            let Ok(gt) =
                std::fs::read_to_string(root.join("groundtruth").join(format!("{name}.json")))
            else {
                continue;
            };
            let bytes = std::fs::read(&path).expect("fixture bytes");
            let doc = PptxBackend
                .convert(&SourceDocument::from_bytes(&name, InputFormat::Pptx, bytes))
                .expect("converts");
            let mut ours: Value = serde_json::from_str(&doc.export_to_json()).unwrap();
            let mut want: Value = serde_json::from_str(&gt).unwrap();
            for v in [&mut ours, &mut want] {
                v.as_object_mut().unwrap().remove("name");
                normalize(v);
            }
            assert_eq!(
                ours, want,
                "{name}: JSON differs from docling's groundtruth"
            );
            compared += 1;
        }
        assert!(compared >= 8, "{compared} fixtures compared");
    }

    /// python-pptx's `Image.dpi`: Pillow's value rounded half-to-even, 72
    /// when absent or outside 1–2048.
    #[test]
    fn dpi_follows_python_pptx() {
        let phys = |ppu: u32| {
            let mut d = ppu.to_be_bytes().to_vec();
            d.extend(ppu.to_be_bytes());
            d.push(1);
            d
        };
        assert_eq!(
            pptx_dpi(&png(&[(b"pHYs", phys(11811))])),
            300,
            "299.9994 rounds up"
        );
        assert_eq!(pptx_dpi(&png(&[(b"pHYs", phys(2835))])), 72, "72.009");
        assert_eq!(
            pptx_dpi(&png(&[(b"pHYs", phys(100_000))])),
            72,
            "2540 is out of range"
        );
        assert_eq!(pptx_dpi(&png(&[(b"IEND", vec![])])), 72, "no pHYs");
        assert_eq!(pptx_dpi(b"GIF89a"), 72);
    }
}
