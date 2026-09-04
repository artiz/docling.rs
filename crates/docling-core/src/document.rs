//! The unified document representation.

use crate::markdown::{to_markdown, to_markdown_images};
use crate::ImageMode;

/// The unified, format-agnostic document produced by every backend.
///
/// This is the heart of docling: backends parse their source format into a
/// `DoclingDocument`, and serializers turn it back into Markdown, HTML, JSON,
/// etc. Phase 0 uses a flat sequence of [`Node`]s; the production schema will
/// match docling-core's body-tree-with-references layout.
#[derive(Debug, Clone, PartialEq)]
pub struct DoclingDocument {
    /// Logical document name (usually the input file stem).
    pub name: String,
    /// Top-level content, in reading order.
    pub nodes: Vec<Node>,
    /// Default Markdown export mode for [`Self::export_to_markdown`]. `false`
    /// (the default) reproduces docling's legacy output byte-for-byte; `true`
    /// emits cleaner, more conformant Markdown. Set by `DocumentConverter`.
    pub strict_markdown: bool,
    /// Emit tables in the compact `| a | b |` / `| - | - |` form rather than
    /// docling-core's width-padded GitHub serializer. The PDF backend sets this
    /// (its committed groundtruth corpus predates the padded serializer); DOCX/HTML
    /// leave it `false` to match current published docling.
    pub compact_tables: bool,
    /// Hyperlinks recovered from the source, as `(anchor_text, href)` pairs in
    /// document order. docling's standard pipeline drops PDF link annotations, so
    /// these are rendered as Markdown `[anchor](href)` **only in strict mode**
    /// (legacy/docling output is left byte-for-byte unchanged). The PDF backend
    /// populates this from pdfium link annotations; other backends leave it empty.
    pub links: Vec<(String, String)>,
    /// Conversion-confidence report (#183), populated by the PDF/image ML
    /// pipeline; `None` for declarative conversions. Deliberately **not**
    /// part of any document export (docling keeps it on the conversion
    /// result, outside the document schema) — docling-serve surfaces it in
    /// the HTTP response instead.
    pub confidence: Option<crate::confidence::ConfidenceReport>,
}

/// A single piece of document content.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A heading. `level` is 1-6.
    Heading { level: u8, text: String },
    /// A run of body text.
    Paragraph { text: String },
    /// A form checkbox (docling's `checkbox_selected`/`checkbox_unselected`): its
    /// clean label `text` with the checked state. DocLang emits a `<checkbox>`
    /// element head; Markdown/JSON render the task-list form (`- [x] `/`- [ ] `).
    CheckboxItem { checked: bool, text: String },
    /// A single list item at the given nesting `level` (0 = top). For ordered
    /// items, `number` is the display number (honoring the list's `start`); it
    /// is unused for unordered items. `first_in_list` marks the first item of a
    /// list so the serializer can blank-line-separate adjacent sibling lists.
    ///
    /// `marker` is the DocLang enumeration marker (`"1."`, `"1.1."`, …) when the
    /// backend provides one — HTML and DOCX set it for enumerated items, so
    /// DocLang emits `<ldiv><marker>…</marker></ldiv>`; Markdown and the other
    /// declarative backends leave it `None`, giving a bare `<ldiv/>` (matching
    /// docling, whose Markdown backend passes no marker).
    ListItem {
        ordered: bool,
        number: u64,
        first_in_list: bool,
        text: String,
        level: u8,
        marker: Option<String>,
        /// Optional layout provenance (`x0,y0,x1,y1`, normalized to 0–511): the
        /// four DocLang `<location>` values emitted inside the `<list>` right
        /// after the item's `<ldiv>`. Set only by backends with real geometry
        /// (e.g. PPTX shapes); `None` for the declarative backends. Kept on the
        /// item itself (rather than a [`Node::Located`] wrapper) so consecutive
        /// items still group into one `<list>`.
        location: Option<[u16; 4]>,
        /// DocLang-only override for items whose DocLang form diverges from their
        /// flat Markdown `text`. Markdown/JSON always render the fields above; the
        /// DocLang serializer, when this is `Some`, takes the list kind, marker,
        /// and content from here instead. Used for docx multilevel numbering
        /// (Markdown shows `- 1.1. x`, DocLang an ordered `<marker>1.1.</marker>`
        /// with clean text) and inline equations/formatting in list items.
        dclx: Option<ListItemDclx>,
        /// The item's hyperlink target, when its content is a link — docling's
        /// HTML backend emits it as an `<href uri=…/>` in the item head, and the
        /// anchor's Markdown link markup is stripped from the rendered content.
        /// `None` for a plain item; ignored by Markdown/JSON.
        href: Option<String>,
        /// Non-body content layer (docling's HTML site chrome before the first
        /// heading → `furniture`). DocLang emits a `<layer value=…/>` in the item
        /// head; Markdown/JSON drop a non-body item entirely.
        layer: Option<ContentLayer>,
    },
    /// A fenced code block.
    Code {
        language: Option<String>,
        text: String,
        /// The original (pre-enrichment) text when the CodeFormula model
        /// rewrote `text`: docling keeps the raw extraction in the JSON `orig`
        /// field while `text` carries the model output. `None` → `orig == text`.
        orig: Option<String>,
        /// A line-preserving rendering, when the backend can reconstruct one
        /// but docling's own output for the format cannot. The PDF pipeline
        /// sets it (docling-parse joins code lines with single spaces, so
        /// `text` carries that flat docling-parity form): **strict** Markdown
        /// prefers `pretty`, every byte-conformance surface (legacy Markdown,
        /// JSON, DocLang, chunks) serializes `text`.
        pretty: Option<String>,
    },
    /// A table. The first row is treated as the header.
    Table(Table),
    /// A picture/figure, with an optional caption and (when a backend extracts
    /// it) the embedded image itself.
    Picture {
        caption: Option<String>,
        /// Hyperlink annotation on the caption (docling's caption text item
        /// `hyperlink`): the HTML backend sets it when an `<a href>` wraps the
        /// image whose `alt` became the caption. DocLang emits the block-form
        /// `<caption>` with an `<href uri=…/>` head; JSON puts `hyperlink` on
        /// the caption item; Markdown and LaTeX print the plain caption text,
        /// as docling does.
        caption_href: Option<String>,
        image: Option<PictureImage>,
        /// DocumentPictureClassifier predictions (all classes, descending
        /// confidence), when the picture-classification enrichment ran.
        /// Serialized as docling's `classification` annotation + `meta` field
        /// on the JSON picture item; Markdown/DocLang output is unaffected.
        classification: Option<Vec<PictureClass>>,
    },
    /// A display-math formula item decoded by the CodeFormula enrichment:
    /// `latex` is the model's LaTeX (no `$$` wrapping), `orig` the raw glyph
    /// text extracted from the PDF. Markdown renders `$$latex$$`; JSON emits a
    /// `formula` text item (docling's un-enriched pipeline instead emits a
    /// placeholder paragraph — see the PDF assembler).
    Formula {
        latex: String,
        orig: String,
        location: Option<[u16; 4]>,
    },
    /// A chart (docling's `PictureItem` classified as a chart, carrying a
    /// `PictureTabularChartData` annotation). Markdown and JSON render it exactly
    /// like a [`Node::Picture`] placeholder (an `<!-- image -->` / `picture`
    /// item); the DocLang serializer emits `<picture class="chart">` with a
    /// `<label value="{kind}"/>` and the data `table` as a `<tabular>`.
    Chart {
        /// docling's classification label, e.g. `bar_chart`, `line_chart`.
        kind: String,
        /// The chart's data grid (row 0 is the header band).
        table: Table,
        /// The chart title (docling's caption item on the picture).
        caption: Option<String>,
        /// DocLang `<location>` provenance for the picture element.
        location: Option<[u16; 4]>,
    },
    /// A logical grouping of child nodes (e.g. a list, a section).
    Group { label: String, children: Vec<Node> },
    /// A form key-value region (docling's `field_region`): a set of form fields,
    /// each pairing an optional marker, key, and value. Backends detect these
    /// from form structure (e.g. HTML's `keyN` / `keyN_valueM` / `keyN_marker`
    /// `id`-convention); the serializers render each item's parts as separate
    /// labelled texts (`marker` / `field_key` / `field_value`).
    FieldRegion { items: Vec<FieldItem> },
    /// Rich inline content — docling's `InlineGroup`: a run of styled text
    /// segments that a backend captured with formatting (`<bold>`, `<italic>`,
    /// `<underline>`, `<strikethrough>`, sub/superscript, inline `<code>`) the
    /// flat Markdown text cannot represent. Markdown/JSON render this exactly
    /// like `Paragraph { text: md_text }` (so their output is unchanged); the
    /// DocLang serializer uses the structured `runs`. `unwrapped` is set when the
    /// group's docling parent is a heading/text (no enclosing `<text>` wrapper).
    InlineGroup {
        unwrapped: bool,
        runs: Vec<InlineRun>,
        md_text: String,
    },
    /// A node in a non-body content layer — `furniture` (page headers/footers,
    /// the HTML `<title>`, site navigation/chrome) or `notes` (docx comments).
    /// Markdown and JSON omit these layers by default; DocLang renders the wrapped
    /// node with a `<layer value="{layer}"/>` head.
    Furniture {
        layer: ContentLayer,
        inner: Box<Node>,
    },
    /// One docx reviewer comment (`w:comment`): docling's notes-layer
    /// `comment_section` group holding a single text item. `name` is docling's
    /// `comment-{id}` — the *docx* comment id, not the ordinal. JSON emits the
    /// group plus its notes-layer text; DocLang emits the flat
    /// `<text><layer value="notes"/>…</text>` upstream writes (its DocLang
    /// carries no group for comments); Markdown and LaTeX omit the notes layer.
    CommentSection { name: String, text: String },
    /// A body item annotated by docx reviewer comments: `comments` are indices
    /// into the document's [`Node::CommentSection`] nodes, in document order.
    /// JSON emits docling's `comments: [{"$ref": "#/groups/N"}]` on the item;
    /// every other serializer renders `inner` unchanged.
    Commented {
        comments: Vec<usize>,
        inner: Box<Node>,
    },
    /// A node carrying layout provenance — the four DocLang `<location>` values
    /// (`x0,y0,x1,y1`, normalized to 0–511) docling attaches to elements from
    /// backends with real geometry (e.g. the slide shapes in PPTX). Markdown and
    /// JSON render the wrapped node unchanged; DocLang emits the `<location>`
    /// tokens as the element's first children.
    Located {
        location: [u16; 4],
        inner: Box<Node>,
    },
    /// A PDF page header or footer (docling's `page_header`/`page_footer`
    /// furniture): DocLang emits `<page_header>`/`<page_footer>` with a
    /// `<layer value="furniture"/>` head, the four `<location>` tokens, then the
    /// text. Markdown and JSON omit it like other furniture.
    PageFurniture {
        footer: bool,
        location: [u16; 4],
        text: String,
    },
    /// A page boundary — docling's implicit page break between pages. The PPTX
    /// backend emits one between consecutive slides. DocLang renders it as
    /// `<page_break/>`; Markdown and JSON omit it (matching docling's default
    /// exports, which carry page breaks only in the document model).
    PageBreak,
    /// An invisible page marker — the first node of every page the PDF paths
    /// assemble: the 1-based page number and the page size in PDF points. It
    /// carries exactly what the JSON export needs to populate docling's
    /// `pages` map and to denormalize the 0–511 `<location>` grid back into
    /// BOTTOMLEFT point bboxes for per-item `prov` (#171). Every other
    /// serializer skips it, so Markdown / DocLang / DocTags output is
    /// byte-for-byte unchanged.
    PageInfo {
        /// 1-based page number (0 = "not yet numbered": the assembler emits
        /// the marker, the document-level collector stamps the real number).
        page_no: usize,
        /// Page width in PDF points.
        width: f32,
        /// Page height in PDF points.
        height: f32,
    },
    /// A node docling keeps in the document model (and DocLang) but leaves out
    /// of the Markdown and JSON exports — e.g. an ODF *presentation*'s pictures
    /// and charts, which appear in the `.dclx` body but not in its `.md`/`.json`.
    /// DocLang renders the wrapped node in place; Markdown and JSON skip it.
    DoclangOnly(Box<Node>),
    /// A verbatim plain-text dump — docling's plain-text backend emits the whole
    /// file as a single text item (used for legacy USPTO APS `.txt` grants, which
    /// docling routes to plain text rather than its APS parser). The stored string
    /// is the file body, one record per line. Markdown/JSON render it as one text
    /// block; the DocLang serializer reproduces minidom's per-line layout, CDATA-
    /// escaping only the lines that need it (see `emit_text_dump`).
    TextDump(String),
}

/// Vertical text position of an [`InlineRun`] — docling's `Script`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Script {
    #[default]
    Baseline,
    Sub,
    Super,
}

/// One styled segment of a [`Node::InlineGroup`] — the docling.rs analogue of a
/// `TextItem` inside an `InlineGroup`, carrying the ancestor formatting docling
/// tracks. `text` is already whitespace-normalized/trimmed (one segment per
/// source text node). A hyperlink is intentionally not stored: DocLang drops the
/// target inside inline scope, keeping only the anchor text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InlineRun {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub script: Script,
    pub code: bool,
    /// An inline equation (`text` holds LaTeX): DocLang renders `<formula>…`,
    /// Markdown/JSON keep the `$…$` already baked into the group's `md_text`.
    pub formula: bool,
}

/// A DocLang content layer other than the default `body` (see [`Node::Furniture`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentLayer {
    /// Page headers/footers, HTML `<title>`, site navigation/chrome.
    Furniture,
    /// Editorial notes (docx reviewer comments).
    Notes,
    /// Invisible content (hidden spreadsheet sheets).
    Invisible,
}

impl ContentLayer {
    /// The `<layer value="…"/>` token value.
    pub fn value(self) -> &'static str {
        match self {
            ContentLayer::Furniture => "furniture",
            ContentLayer::Notes => "notes",
            ContentLayer::Invisible => "invisible",
        }
    }
}

/// DocLang-only content for a [`Node::ListItem`] whose DocLang form differs from
/// its flat Markdown `text` (see [`Node::ListItem::dclx`]). `ordered` picks the
/// enclosing `<list>` kind, `marker` the `<ldiv><marker>`; content is `runs`
/// (structured equations/formatting) when non-empty, else `text` re-parsed for
/// inline markers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListItemDclx {
    pub ordered: bool,
    pub marker: Option<String>,
    pub text: String,
    pub runs: Vec<InlineRun>,
}

impl InlineRun {
    /// A run with no active formatting (renders as bare inline text).
    pub fn is_plain(&self) -> bool {
        !self.bold
            && !self.italic
            && !self.underline
            && !self.strike
            && !self.code
            && !self.formula
            && self.script == Script::Baseline
    }
}

/// Build the [`Node`] for a paragraph of inline content from its structured
/// `runs` and Markdown text, applying docling's `InlineGroup` boundary:
///
/// * a single plain run (or none) → a plain [`Node::Paragraph`] (which the
///   serializers render as `<text>…</text>`, and a lone hyperlink via `<href>`);
/// * a single uniformly-formatted run, or two or more runs → a
///   [`Node::InlineGroup`]. `unwrapped` (the group's docling parent is a
///   heading, so no enclosing `<text>`) only applies to multi-run groups.
///
/// Markdown/JSON render the group's `md_text`, so their output is identical to
/// emitting a `Paragraph` — the structured runs are DocLang-only.
pub fn inline_paragraph_node(md_text: String, runs: Vec<InlineRun>, unwrapped: bool) -> Node {
    let single_plain = runs.len() <= 1 && runs.first().is_none_or(|r| r.is_plain());
    if single_plain {
        Node::Paragraph { text: md_text }
    } else {
        Node::InlineGroup {
            unwrapped: unwrapped && runs.len() >= 2,
            runs,
            md_text,
        }
    }
}

/// One entry of a [`Node::FieldRegion`]: a marker/key/value triple, any of which
/// may be absent. Mirrors docling's `field_item` with its `marker` / `field_key`
/// / `field_value` child texts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FieldItem {
    pub marker: Option<String>,
    pub key: Option<String>,
    pub value: Option<String>,
}

/// One DocumentPictureClassifier prediction — docling-core's
/// `PictureClassificationClass` (`class_name` + `confidence`).
#[derive(Debug, Clone, PartialEq)]
pub struct PictureClass {
    /// e.g. `bar_chart`, `logo`, `signature` (the classifier's 26-label set).
    pub class_name: String,
    pub confidence: f32,
}

/// An extracted picture's raw encoded bytes plus its mimetype and pixel size —
/// the docling.rs analogue of docling-core's `ImageRef`.
#[derive(Debug, Clone, PartialEq)]
pub struct PictureImage {
    /// e.g. `image/png`, `image/jpeg`.
    pub mimetype: String,
    pub width: u32,
    pub height: u32,
    /// The image file bytes, exactly as embedded (PNG/JPEG/…).
    pub data: Vec<u8>,
}

impl PictureImage {
    /// A `data:` URI for the image (`data:<mimetype>;base64,<…>`).
    pub fn data_uri(&self) -> String {
        format!(
            "data:{};base64,{}",
            self.mimetype,
            crate::base64::encode(&self.data)
        )
    }
}

/// One table cell as a first-class object (#240) — the Rust counterpart of
/// docling's `TableCell`: its text, page geometry, grid rectangle and header
/// roles. Produced by the PDF TableFormer paths from the predicted OTSL
/// structure; `bbox` is `[l, t, r, b]` in page points with a top-left origin.
#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    pub text: String,
    /// `[l, t, r, b]`, page points, top-left origin; `None` without geometry.
    pub bbox: Option<[f32; 4]>,
    /// Anchor grid position (0-based row/column offsets).
    pub start_row: usize,
    pub start_col: usize,
    /// Span extents (≥ 1); the covered grid positions repeat the cell's text
    /// in [`Table::rows`].
    pub row_span: usize,
    pub col_span: usize,
    /// OTSL `ched` — a column-header cell.
    pub column_header: bool,
    /// OTSL `rhed` — a row-header cell.
    pub row_header: bool,
    /// OTSL `srow` — a section-row cell.
    pub row_section: bool,
}

/// A simple row-major table. By default `rows[0]` is the header row; a
/// [`TableStructure`] overlay overrides that and adds column spans.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table {
    pub rows: Vec<Vec<String>>,
    /// Optional layout provenance: the four DocLang `<location>` values
    /// (`x0,y0,x1,y1`, each already normalized to the 0–511 resolution) emitted
    /// before the table's cells. Set only by backends with real geometry (e.g.
    /// the spreadsheet backend, whose cell grid yields a bounding box); left
    /// `None` by declarative backends, which have no coordinates.
    pub location: Option<[u16; 4]>,
    /// Optional OTSL structure overlay for backends that parse real table
    /// geometry (USPTO CALS): explicit header-row count and horizontal-span
    /// continuations. `None` → the default (row 0 is the header, no spans).
    /// `rows` still carries the full text grid (span text replicated) for
    /// Markdown/JSON; DocLang uses this overlay to emit `<ched/>`/`<lcel/>`.
    pub structure: Option<TableStructure>,
    /// Optional per-cell block content, parallel to `rows`. A *rich* cell (an
    /// ODF cell holding a list, several paragraphs, or a nested table) carries
    /// its DocLang blocks here; the DocLang serializer emits them after the
    /// cell token instead of the flat `rows` text. Markdown/JSON ignore this
    /// and render `rows`, so their output is unchanged. `None` (or an empty
    /// `Vec` for a given cell) → the flat text is used everywhere.
    pub cell_blocks: Option<Vec<Vec<Vec<Node>>>>,
    /// Optional caption (docling's `TableItem.captions`): the JATS
    /// `<table-wrap>` label+caption, an HTML `<caption>`, etc. Markdown renders
    /// it as a text line *before* the grid; JSON emits a caption text item the
    /// table references; DocLang emits a `<caption>` as the table's first child.
    /// `None` → the table has no caption.
    pub caption: Option<String>,
    /// Optional per-cell bounding boxes, same shape as [`Self::rows`]: `[l, t,
    /// r, b]` in page points with a **top-left** origin (the PDF pipeline's
    /// native space). Set by the ML pipeline's TableFormer paths — a spanned
    /// cell repeats its anchor's box across the covered grid positions — and
    /// First-class cells (#240): the authoritative per-cell records —
    /// text, page geometry, spans and header roles — when the backend
    /// produces them (the PDF TableFormer paths do; declarative backends
    /// leave `None`). [`Self::rows`] stays the dense text grid every
    /// serializer renders (a spanning cell's text is replicated across its
    /// covered positions there); JSON serializes these cells verbatim when
    /// present, and the DocLang structure overlay is derived from them.
    pub cells: Option<Vec<TableCell>>,
}

impl Table {
    /// A cell's text at a grid position, `None` outside the grid.
    pub fn cell_text(&self, row: usize, col: usize) -> Option<&str> {
        self.rows.get(row)?.get(col).map(String::as_str)
    }

    /// Replace the text at a grid position; `false` (and no change) outside
    /// the grid. When a first-class cell covers the position, the whole
    /// cell is updated: its record text and every grid position its span
    /// covers, so the repair shows once in Markdown, not once per covered
    /// column.
    pub fn set_cell_text(&mut self, row: usize, col: usize, text: impl Into<String>) -> bool {
        if self.rows.get(row).and_then(|r| r.get(col)).is_none() {
            return false;
        }
        let text = text.into();
        let covering = self.cells.as_mut().and_then(|cells| {
            cells.iter_mut().find(|c| {
                (c.start_row..c.start_row + c.row_span).contains(&row)
                    && (c.start_col..c.start_col + c.col_span).contains(&col)
            })
        });
        if let Some(cell) = covering {
            cell.text = text.clone();
            let (r0, r1) = (cell.start_row, cell.start_row + cell.row_span);
            let (c0, c1) = (cell.start_col, cell.start_col + cell.col_span);
            for r in self.rows.iter_mut().take(r1).skip(r0) {
                for slot in r.iter_mut().take(c1).skip(c0) {
                    *slot = text.clone();
                }
            }
        } else {
            self.rows[row][col] = text;
        }
        true
    }

    /// Derive first-class cells (#240) from the dense grid plus the
    /// [`TableStructure`] overlay — how declarative tables (DOCX/XLSX merged
    /// regions, HTML `th`/spans, ODF covered cells, USPTO CALS) get real
    /// `TableCell` records without page geometry. Anchors are the positions
    /// not marked as span continuations; extents scan the continuation grids
    /// right/down (matching the DocLang `lcel`/`ucel` reading). Header roles
    /// come from the per-cell `col_header`/`row_header` grids when present,
    /// else the `header_row` band, else docling's declarative default (row 0
    /// is the header). Without any overlay every position is a 1×1 cell.
    pub fn derive_cells(&self) -> Vec<TableCell> {
        let s = self.structure.as_ref();
        let flag = |grid: Option<&Vec<Vec<bool>>>, r: usize, c: usize| {
            grid.and_then(|g| g.get(r))
                .and_then(|row| row.get(c))
                .copied()
                .unwrap_or(false)
        };
        let col_cont = |r: usize, c: usize| flag(s.map(|s| &s.col_continuation), r, c);
        let row_cont = |r: usize, c: usize| flag(s.map(|s| &s.row_continuation), r, c);
        let is_col_header = |r: usize, c: usize| match s {
            Some(st) if !st.col_header.is_empty() => flag(Some(&st.col_header), r, c),
            Some(st) if !st.header_row.is_empty() => st.header_row.get(r).copied().unwrap_or(false),
            _ => r == 0,
        };
        let mut cells = Vec::new();
        for (r, row) in self.rows.iter().enumerate() {
            for (c, text) in row.iter().enumerate() {
                if col_cont(r, c) || row_cont(r, c) {
                    continue; // covered by a span anchor
                }
                let mut col_span = 1;
                while c + col_span < row.len() && col_cont(r, c + col_span) {
                    col_span += 1;
                }
                let mut row_span = 1;
                while r + row_span < self.rows.len() && row_cont(r + row_span, c) {
                    row_span += 1;
                }
                cells.push(TableCell {
                    text: text.clone(),
                    bbox: None,
                    start_row: r,
                    start_col: c,
                    row_span,
                    col_span,
                    column_header: is_col_header(r, c),
                    row_header: flag(s.map(|s| &s.row_header), r, c),
                    row_section: false,
                });
            }
        }
        cells
    }

    /// The first-class cell covering a grid position, if any.
    pub fn cell_at(&self, row: usize, col: usize) -> Option<&TableCell> {
        self.cells.as_ref()?.iter().find(|c| {
            (c.start_row..c.start_row + c.row_span).contains(&row)
                && (c.start_col..c.start_col + c.col_span).contains(&col)
        })
    }

    /// A cell's bounding box (`[l, t, r, b]`, page points, top-left origin);
    /// `None` when no cell with geometry covers the position.
    pub fn cell_bbox(&self, row: usize, col: usize) -> Option<[f32; 4]> {
        self.cell_at(row, col)?.bbox
    }

    /// Set (or replace) the bounding box of the cell covering a grid
    /// position; `false` outside the text grid. A table without first-class
    /// cells materializes them first (one 1×1 cell per grid position, texts
    /// from the grid), so declarative tables can be annotated too.
    pub fn set_cell_bbox(&mut self, row: usize, col: usize, bbox: [f32; 4]) -> bool {
        if self.rows.get(row).and_then(|r| r.get(col)).is_none() {
            return false;
        }
        let rows = &self.rows;
        let cells = self.cells.get_or_insert_with(|| {
            rows.iter()
                .enumerate()
                .flat_map(|(r, cols)| {
                    cols.iter().enumerate().map(move |(c, text)| TableCell {
                        text: text.clone(),
                        bbox: None,
                        start_row: r,
                        start_col: c,
                        row_span: 1,
                        col_span: 1,
                        column_header: false,
                        row_header: false,
                        row_section: false,
                    })
                })
                .collect()
        });
        match cells.iter_mut().find(|c| {
            (c.start_row..c.start_row + c.row_span).contains(&row)
                && (c.start_col..c.start_col + c.col_span).contains(&col)
        }) {
            Some(cell) => {
                cell.bbox = Some(bbox);
                true
            }
            None => {
                cells.push(TableCell {
                    text: self.rows[row][col].clone(),
                    bbox: Some(bbox),
                    start_row: row,
                    start_col: col,
                    row_span: 1,
                    col_span: 1,
                    column_header: false,
                    row_header: false,
                    row_section: false,
                });
                true
            }
        }
    }

    /// The anchor position of the cell whose box overlaps `bbox` best
    /// (largest intersection-over-union), ties resolved in cell order.
    /// `None` when nothing overlaps or the table carries no geometry. This
    /// is the lookup half of the repair workflow: find the cell an external
    /// OCR box refers to, then [`Self::set_cell_text`] it.
    pub fn find_cell_by_bbox(&self, bbox: [f32; 4]) -> Option<(usize, usize)> {
        let area = |b: &[f32; 4]| ((b[2] - b[0]) * (b[3] - b[1])).max(0.0);
        let mut best: Option<(f32, (usize, usize))> = None;
        for cell in self.cells.as_deref()?.iter() {
            let Some(cb) = cell.bbox else { continue };
            let iw = (bbox[2].min(cb[2]) - bbox[0].max(cb[0])).max(0.0);
            let ih = (bbox[3].min(cb[3]) - bbox[1].max(cb[1])).max(0.0);
            let inter = iw * ih;
            if inter <= 0.0 {
                continue;
            }
            let iou = inter / (area(&bbox) + area(&cb) - inter).max(f32::EPSILON);
            if best.is_none_or(|(b, _)| iou > b) {
                best = Some((iou, (cell.start_row, cell.start_col)));
            }
        }
        best.map(|(_, pos)| pos)
    }

    /// Locate the cell overlapping `bbox` best and replace its text — the
    /// one-call form of the OCR-repair loop. Returns the updated anchor.
    pub fn update_cell_by_bbox(
        &mut self,
        bbox: [f32; 4],
        text: impl Into<String>,
    ) -> Option<(usize, usize)> {
        let (row, col) = self.find_cell_by_bbox(bbox)?;
        self.set_cell_text(row, col, text);
        Some((row, col))
    }
}

/// OTSL structure overlay for a [`Table`], parallel to [`Table::rows`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableStructure {
    /// Per-row: `true` if the row's non-empty cells are column headers
    /// (emitted as `<ched/>` rather than `<fcel/>`).
    pub header_row: Vec<bool>,
    /// Same shape as [`Table::rows`]; `true` where a cell continues a
    /// horizontal span from its left neighbour (emitted as `<lcel/>`).
    pub col_continuation: Vec<Vec<bool>>,
    /// Same shape as [`Table::rows`]; `true` where a cell continues a
    /// vertical span from the cell above (emitted as `<ucel/>`). Empty or all
    /// `false` when the backend has no vertical spans (e.g. USPTO CALS).
    pub row_continuation: Vec<Vec<bool>>,
    /// Same shape as [`Table::rows`]; `true` where a non-empty cell is a row
    /// header (emitted as `<rhed/>`) — a chart's category column. Empty when
    /// the table has no row headers.
    pub row_header: Vec<Vec<bool>>,
    /// Same shape as [`Table::rows`]; `true` where a cell is a *column header*
    /// cell (an HTML `<th>`). When non-empty this per-cell grid supersedes the
    /// per-row [`Self::header_row`] for `<ched/>` emission, matching docling's
    /// cell-level `column_header` flag; the chunker derives its header-row
    /// count from it.
    pub col_header: Vec<Vec<bool>>,
}

impl DoclingDocument {
    /// Create an empty document with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            nodes: Vec::new(),
            strict_markdown: false,
            compact_tables: false,
            links: Vec::new(),
            confidence: None,
        }
    }

    /// Append a node.
    /// The document's top-level tables in reading order — the read half of
    /// the post-extraction table API (#238). [`Node::Located`] wrappers (the
    /// PDF pipeline attaches layout provenance that way) are looked through;
    /// tables nested inside rich table cells (`Table::cell_blocks`) are not
    /// traversed.
    pub fn tables(&self) -> impl Iterator<Item = &Table> {
        fn unwrap_table(n: &Node) -> Option<&Table> {
            match n {
                Node::Table(t) => Some(t),
                Node::Located { inner, .. } => unwrap_table(inner),
                _ => None,
            }
        }
        self.nodes.iter().filter_map(unwrap_table)
    }

    /// Mutable access to the document's top-level tables, for repair
    /// workflows (#238): locate a cell via [`Table::find_cell_by_bbox`], fix
    /// its text with [`Table::set_cell_text`], then re-export — every
    /// serializer reads the same grid.
    pub fn tables_mut(&mut self) -> impl Iterator<Item = &mut Table> {
        fn unwrap_table(n: &mut Node) -> Option<&mut Table> {
            match n {
                Node::Table(t) => Some(t),
                Node::Located { inner, .. } => unwrap_table(inner),
                _ => None,
            }
        }
        self.nodes.iter_mut().filter_map(unwrap_table)
    }

    pub fn push(&mut self, node: Node) {
        self.nodes.push(node);
    }

    /// Convenience: append a heading.
    pub fn add_heading(&mut self, level: u8, text: impl Into<String>) {
        self.push(Node::Heading {
            level,
            text: text.into(),
        });
    }

    /// Convenience: append a paragraph.
    pub fn add_paragraph(&mut self, text: impl Into<String>) {
        self.push(Node::Paragraph { text: text.into() });
    }

    /// Serialize the document to Markdown.
    ///
    /// The Rust equivalent of docling-core's
    /// `DoclingDocument.export_to_markdown()`. Uses [`Self::strict_markdown`] to
    /// pick between docling-legacy output (default) and the cleaner, more
    /// conformant variant.
    pub fn export_to_markdown(&self) -> String {
        to_markdown(self, self.strict_markdown)
    }

    /// Serialize to Markdown, explicitly choosing the mode regardless of
    /// [`Self::strict_markdown`]. `strict = true` produces cleaner, more
    /// conformant Markdown (code-fence languages preserved, no inline-run
    /// spacing artifacts); `strict = false` reproduces docling's legacy output.
    pub fn export_to_markdown_with(&self, strict: bool) -> String {
        to_markdown(self, strict)
    }

    /// Markdown for this document as the *content of a rich table cell*
    /// (docling-core's `in_table_cell` serialization, docling-core#540):
    /// headings render as plain text since Markdown tables can't hold them.
    /// Backends build a sub-document per rich cell and flatten this into the
    /// cell text; no trailing newline.
    pub fn export_to_table_cell_markdown(&self) -> String {
        crate::markdown::to_markdown_table_cell(self, self.strict_markdown)
    }

    /// Serialize to docling-core's native JSON wire format (`DoclingDocument`
    /// schema), pretty-printed — the Rust equivalent of
    /// `DoclingDocument.export_to_dict()` / `save_as_json()`. The output loads
    /// back into Python docling-core and round-trips to the same Markdown.
    pub fn export_to_json(&self) -> String {
        serde_json::to_string_pretty(&self.export_to_json_value())
            .expect("DoclingDocument JSON is always serializable")
    }

    /// The same JSON wire format as [`Self::export_to_json`], as a
    /// `serde_json::Value` — for callers that append response-level extras
    /// (docling-serve adds the confidence report, #183) before serializing.
    pub fn export_to_json_value(&self) -> serde_json::Value {
        crate::json::to_json(self)
    }

    /// Serialize to a complete LaTeX document — the Rust counterpart of
    /// docling-core's `LaTeXDocSerializer` with default parameters (docling
    /// 2.124's `--to latex`, #317). No trailing newline, like the upstream
    /// CLI's `<stem>.tex`.
    pub fn export_to_latex(&self) -> String {
        crate::latex::to_latex(self)
    }

    /// Serialize to DocLang XML (`<doclang version="0.7">…`), the markup that
    /// lives inside a `.dclx` archive — the Rust counterpart of docling-core's
    /// `export_to_doclang()` with default parameters. No trailing newline; the
    /// archive writer appends exactly one.
    pub fn export_to_doclang(&self) -> String {
        crate::doclang::export_to_doclang(&self.nodes)
    }

    /// Serialize to Markdown with an explicit picture [`ImageMode`] (mirrors
    /// docling's `image_mode`). Returns the Markdown and, for
    /// [`ImageMode::Referenced`], the `(relative-path, bytes)` of each image the
    /// caller should write next to the Markdown file. `artifacts_dir` is the
    /// directory name used in referenced links.
    pub fn export_to_markdown_with_images(
        &self,
        image_mode: ImageMode,
        artifacts_dir: &str,
    ) -> (String, Vec<(String, Vec<u8>)>) {
        to_markdown_images(self, self.strict_markdown, image_mode, artifacts_dir)
    }
}

#[cfg(test)]
mod table_api_tests {
    use super::*;

    fn cell(
        text: &str,
        bbox: [f32; 4],
        (start_row, start_col): (usize, usize),
        (row_span, col_span): (usize, usize),
    ) -> TableCell {
        TableCell {
            text: text.into(),
            bbox: Some(bbox),
            start_row,
            start_col,
            row_span,
            col_span,
            column_header: false,
            row_header: false,
            row_section: false,
        }
    }

    fn table() -> Table {
        Table {
            rows: vec![
                vec!["Year".into(), "Ducks".into()],
                vec!["2019".into(), "120".into()],
            ],
            cells: Some(vec![
                cell("Year", [0.0, 0.0, 50.0, 10.0], (0, 0), (1, 1)),
                cell("Ducks", [50.0, 0.0, 100.0, 10.0], (0, 1), (1, 1)),
                cell("2019", [0.0, 10.0, 50.0, 20.0], (1, 0), (1, 1)),
                cell("120", [50.0, 10.0, 100.0, 20.0], (1, 1), (1, 1)),
            ]),
            ..Default::default()
        }
    }

    /// Declarative tables derive first-class cells from the structure
    /// overlay: continuation grids become span extents, `col_header` (or the
    /// row-0 fallback) becomes the header role — the XLSX/DOCX/HTML merge
    /// path into real `TableCell`s (#240).
    #[test]
    fn derive_cells_reads_spans_and_headers_from_structure() {
        let t = Table {
            rows: vec![
                vec!["Wide".into(), "Wide".into(), "C".into()],
                vec!["a".into(), "b".into(), "c".into()],
            ],
            structure: Some(TableStructure {
                header_row: vec![true, false],
                col_continuation: vec![vec![false, true, false], vec![false; 3]],
                row_continuation: vec![vec![false; 3], vec![false; 3]],
                row_header: Vec::new(),
                col_header: Vec::new(),
            }),
            ..Default::default()
        };
        let cells = t.derive_cells();
        assert_eq!(cells.len(), 5, "two anchors in row 0, three in row 1");
        let wide = &cells[0];
        assert_eq!((wide.col_span, wide.row_span), (2, 1));
        assert!(wide.column_header, "header_row band");
        assert!(cells.iter().skip(2).all(|c| !c.column_header));

        // Without any overlay: every position 1x1, row 0 the header
        // (docling's declarative default — the old JSON synthesis).
        let plain = Table {
            rows: vec![vec!["h".into()], vec!["x".into()]],
            ..Default::default()
        };
        let cells = plain.derive_cells();
        assert_eq!(cells.len(), 2);
        assert!(cells[0].column_header && !cells[1].column_header);
    }

    /// A spanning cell updates once: the record text and every covered grid
    /// position — a repair shows once in Markdown, not once per column.
    #[test]
    fn span_repair_updates_the_whole_cell() {
        let mut t = Table {
            rows: vec![
                vec!["Wide".into(), "Wide".into(), "C".into()],
                vec!["a".into(), "b".into(), "c".into()],
            ],
            cells: Some(vec![
                cell("Wide", [0.0, 0.0, 100.0, 10.0], (0, 0), (1, 2)),
                cell("C", [100.0, 0.0, 150.0, 10.0], (0, 2), (1, 1)),
            ]),
            ..Default::default()
        };
        // Update through the covered (non-anchor) position.
        assert!(t.set_cell_text(0, 1, "Fixed"));
        assert_eq!(
            t.rows[0],
            vec!["Fixed".to_string(), "Fixed".into(), "C".into()]
        );
        assert_eq!(t.cell_at(0, 1).unwrap().text, "Fixed");
        assert_eq!(t.cell_at(0, 1).unwrap().col_span, 2);
    }

    /// The OCR-repair loop (#238): locate a cell by an external box (best
    /// IoU), replace its text, and see the fix in the export — the grid is
    /// the single source of truth for every serializer.
    #[test]
    fn bbox_lookup_and_repair_flow_into_exports() {
        let mut doc = DoclingDocument::new("t");
        doc.push(Node::Table(table()));
        assert_eq!(doc.tables().count(), 1);

        let t = doc.tables_mut().next().unwrap();
        // A slightly-off OCR box still lands on the (1,1) cell.
        assert_eq!(t.find_cell_by_bbox([52.0, 11.0, 98.0, 19.0]), Some((1, 1)));
        assert_eq!(
            t.update_cell_by_bbox([52.0, 11.0, 98.0, 19.0], "125"),
            Some((1, 1))
        );
        assert_eq!(t.cell_text(1, 1), Some("125"));
        assert!(doc.export_to_markdown().contains("125"));

        // No overlap → no match, nothing changed.
        let t = doc.tables_mut().next().unwrap();
        assert_eq!(t.find_cell_by_bbox([500.0, 500.0, 600.0, 600.0]), None);
    }

    #[test]
    fn cell_accessors_bound_check_and_geometry_materializes() {
        let mut t = table();
        assert_eq!(t.cell_text(0, 0), Some("Year"));
        assert_eq!(t.cell_text(5, 0), None);
        assert!(!t.set_cell_text(0, 9, "x"), "outside the grid");
        assert_eq!(t.cell_bbox(1, 0), Some([0.0, 10.0, 50.0, 20.0]));

        // A geometry-less table materializes its box grid on first set.
        let mut plain = Table {
            rows: vec![vec!["a".into(), "b".into()]],
            ..Default::default()
        };
        assert_eq!(plain.cell_bbox(0, 1), None);
        assert!(!plain.set_cell_bbox(0, 5, [0.0; 4]), "outside the grid");
        assert!(plain.set_cell_bbox(0, 1, [1.0, 2.0, 3.0, 4.0]));
        assert_eq!(plain.cell_bbox(0, 1), Some([1.0, 2.0, 3.0, 4.0]));
        assert_eq!(plain.find_cell_by_bbox([1.5, 2.5, 2.5, 3.5]), Some((0, 1)));
    }
}
