//! The one set of conversion options every surface speaks (#577).
//!
//! [`ConvertOptions`] is the serializable, surface-neutral form of what
//! configures a [`DocumentConverter`]: every knob of its builder plus the
//! pipeline selection (`standard` | `vlm`) and the `vlm_*` settings. The CLI
//! fills it from flags, docling-serve deserializes it from the query string /
//! JSON body / multipart text parts, the C ABI and the wasm module take it as
//! one JSON object, the Python and Node bindings map their keyword arguments
//! into it — and all of them then go through the same two methods:
//! [`ConvertOptions::validate`] (one set of rejection rules, one set of
//! messages) and [`ConvertOptions::apply`] (one mapping onto the builder).
//! Before this module each surface re-declared the fields, re-implemented
//! the checks and re-mapped the names, so a new option meant edits in a dozen
//! files and the surfaces drifted (the C ABI had no `list_attachments`, the
//! Node bindings never rejected `vlmMaxTokens: 0`, …).
//!
//! Every field is an `Option`: `None` means "not given", and the engine's
//! own default — defined once, in [`DocumentConverter::default`] — applies.
//! That is what lets docling-serve layer its three option sources
//! ([`merge_options`]: a later layer's `Some` wins, its `None` keeps the
//! earlier value) and what lets a binding start from a pre-configured base
//! converter and apply only what the caller said.
//!
//! The field names are the wire names: docling-serve's request options, the
//! C ABI's and the wasm module's JSON keys, the CLI flags with `_` → `-`, the
//! Node options in camelCase (napi's rename), the Python keyword arguments.
//! The few places a surface spells one differently are recorded in
//! [`OPTIONS`], the table the inventory test
//! (`crates/docling/tests/options_inventory.rs`) checks every surface's
//! documentation against — a field added here without its row, flag, kwarg
//! or OpenAPI entry fails that test rather than shipping unreachable.
//!
//! Not in here: what to *emit* (`to`, the image mode, the Pandoc API version,
//! the chunker settings — [`crate::chunks::ChunkOptions`]) and where. Those
//! depend on what the surface can do with the result (serve streams, the
//! browser cannot write files, the CLI writes several formats at once), so
//! each surface keeps its own output options next to this struct.

use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::DocumentConverter;

/// Conversion options as every surface accepts them — see the module docs.
/// `None` everywhere is the engine's defaults.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConvertOptions {
    // --- output shaping carried on the document -----------------------------
    /// Cleaner, more conformant Markdown ([`DocumentConverter::strict`]).
    pub strict: Option<bool>,
    /// Compact `| a | b |` Markdown tables instead of width padding (#271).
    pub compact_tables: Option<bool>,
    /// Text inserted between pages in the Markdown export (docling-core's
    /// `MarkdownParams.page_break_placeholder`); docling-serve's historical
    /// spelling `md_page_break_placeholder` is accepted as an alias.
    #[serde(alias = "md_page_break_placeholder")]
    pub page_break_placeholder: Option<String>,

    // --- declarative formats -------------------------------------------------
    /// Resolve external `<img src>` for HTML/EPUB/MHTML/JATS (network access).
    pub fetch_images: Option<bool>,
    /// Email (.eml/.msg): append an Attachments section (#251).
    pub list_attachments: Option<bool>,
    /// Omit empty cells from sparse XLSX/XLS table grids (#271).
    pub skip_empty_cells: Option<bool>,
    /// EBCDIC copybook layout (#252): inline JSON or a file path.
    pub ebcdic_layout: Option<String>,
    /// Character encoding of text inputs (a WHATWG label); unset = detect.
    pub encoding: Option<String>,
    /// Pre-render HTML with a headless browser (the `web-browser` feature).
    pub use_web_browser: Option<bool>,
    /// XBRL: the directory the instance's taxonomy is read from.
    pub xbrl_taxonomy: Option<String>,

    // --- audio / video -------------------------------------------------------
    /// ASR preset (`whisper_*`, `parakeet_tdt_0.6b_v3`); unset = Whisper tiny.
    pub asr_model: Option<String>,
    /// Transcription language (a Whisper code) or `auto`.
    pub asr_lang: Option<String>,
    /// Max frames sampled from a video (0 = transcript only); unset =
    /// [`crate::DEFAULT_VIDEO_FRAMES`].
    pub video_frames: Option<usize>,

    // --- PDF / image pipeline ------------------------------------------------
    /// PDF page window, `"A-B"` or a single `"N"` (1-based inclusive, #80).
    pub pages: Option<String>,
    /// Per-document budget in seconds for the PDF pipeline (#497).
    pub document_timeout: Option<f64>,
    /// Skip the whole ML stack: text layer only (#611; `no_ocr` before 2.0).
    pub text_layer_only: Option<bool>,
    /// Keep layout + TableFormer, never run OCR — docling's `do_ocr=False`,
    /// its CLI's `--no-ocr` (#611).
    pub no_ocr: Option<bool>,
    /// `no_ocr` under its pre-2.0 name (#244), still read: either one set
    /// skips OCR ([`Self::ocr_disabled`]). A field of its own rather than a
    /// serde alias, so a body that sends both spellings (a pre-2.0 form)
    /// is not a duplicate-field error.
    pub skip_ocr: Option<bool>,
    /// The password of an encrypted PDF or Office document (#611, #625).
    /// `pdf_password`, docling's PDF-only name (`--pdf-password`), is read
    /// as an alias.
    #[serde(alias = "pdf_password")]
    pub password: Option<String>,
    /// OCR every page, discarding the text layer.
    pub force_full_page_ocr: Option<bool>,
    /// Skip TableFormer (geometric tables instead).
    pub no_table_former: Option<bool>,
    /// Disable the text-panel heuristic (#173).
    pub no_text_panels: Option<bool>,
    /// Infer heading levels after assembly (#302).
    pub heading_hierarchy: Option<bool>,
    /// `ppocr` (default) | `tesseract` (#460).
    pub ocr_engine: Option<String>,
    /// OCR language: `en` | `ch` or a BCP-47 tag under PP-OCR, tessdata stems
    /// or BCP-47 tags under Tesseract.
    pub ocr_lang: Option<String>,
    /// docling's `OcrMode` (#254): `default` | `full_page` | `layout_regions`
    /// | `pdf_aware_layout_regions`.
    pub ocr_mode: Option<String>,
    /// OCR input scale in px per PDF point (#254); unset = the 2.0 render.
    pub ocr_scale: Option<f32>,
    /// Picture-crop / page-image scale in px per point, 0.1–4.0 (#520).
    pub images_scale: Option<f32>,
    /// Keep each page's render as the JSON page image (docling's
    /// `generate_page_images`, #520 — accepted as an alias).
    #[serde(alias = "generate_page_images")]
    pub page_images: Option<bool>,
    /// Enrichment: classify pictures with DocumentFigureClassifier (#423).
    pub do_picture_classification: Option<bool>,
    /// Enrichment: rewrite code blocks with CodeFormulaV2 (#423).
    pub do_code_enrichment: Option<bool>,
    /// Enrichment: decode display formulas to LaTeX with CodeFormulaV2 (#423).
    pub do_formula_enrichment: Option<bool>,
    /// Enrichment: OCR the embedded pictures of non-PDF documents (#645);
    /// the text becomes the picture's description annotation.
    pub do_picture_ocr: Option<bool>,
    /// Comma-separated DocumentFigureClassifier labels a picture must be
    /// classified as to be OCR'd (#645); unset/empty = every picture.
    pub picture_ocr_classes: Option<String>,
    /// Smallest side in px a picture must have to be OCR'd (#645; default
    /// 32, `DOCLING_RS_PICTURE_OCR_MIN_SIDE`).
    pub picture_ocr_min_side: Option<u32>,
    /// Keep the embedded image bytes on the pictures (#645); `false` drops
    /// them after the enrichment pass. Default `true`.
    pub keep_picture_images: Option<bool>,

    // --- pipeline selection --------------------------------------------------
    /// `standard` (default) | `vlm` (#77): the remote vision model instead of
    /// the local ML stack. The `vlm_*` options are inert under `standard`.
    pub pipeline: Option<String>,
    /// VLM endpoint; falls back to `DOCLING_RS_VLM_ENDPOINT`.
    pub vlm_endpoint: Option<String>,
    /// VLM model name; falls back to `DOCLING_RS_VLM_MODEL`.
    pub vlm_model: Option<String>,
    /// Bearer token for the endpoint; falls back to `DOCLING_RS_VLM_API_KEY`.
    pub vlm_api_key: Option<String>,
    /// Per-page instruction; falls back to `DOCLING_RS_VLM_PROMPT`.
    pub vlm_prompt: Option<String>,
    /// `max_tokens` per completion (default 8192); 0 is rejected.
    pub vlm_max_tokens: Option<usize>,
}

/// A rejected option: which field, and why. The message names the field in
/// its wire spelling (`ocr_scale must be a positive number, got 0`); the CLI
/// prints it through [`OptionsError::cli_message`], which swaps in the flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionsError {
    /// The wire name of the offending option — a [`ConvertOptions`] field.
    pub field: &'static str,
    /// Why it was rejected; names the field.
    pub message: String,
}

impl OptionsError {
    fn new(field: &'static str, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }

    /// The message with the field spelled as its CLI flag (`--ocr-scale must
    /// be a positive number, got 0`).
    pub fn cli_message(&self) -> String {
        let flag = cli_flag(self.field);
        if self.message.contains(self.field) {
            self.message.replacen(self.field, &flag, 1)
        } else {
            format!("{flag}: {}", self.message)
        }
    }
}

impl fmt::Display for OptionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OptionsError {}

/// Which conversion pipeline an option set selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineKind {
    /// The local ML stack (layout, TableFormer, OCR) — the default.
    Standard,
    /// A remote OpenAI-compatible vision model (#77).
    Vlm,
}

/// How one option is spelled on each surface, where it differs from the wire
/// name — the central documentation the inventory test holds every surface
/// to. One row per [`ConvertOptions`] field (the test checks both ways).
#[derive(Debug, Clone, Copy)]
pub struct OptionInfo {
    /// The field / wire name.
    pub name: &'static str,
    /// Other wire spellings accepted on deserialization (`serde(alias)`).
    pub aliases: &'static [&'static str],
    /// The CLI flag; `None` = the default `--name-with-dashes`.
    pub cli: Option<&'static str>,
    /// The Python `DocumentConverter(...)` keyword argument; `Some("")` when
    /// the Python surface has no such argument (the reason is in the row's
    /// comment); `None` = the field name itself.
    pub python: Option<&'static str>,
    /// The Node option (napi spells the field in camelCase); `Some("")` when
    /// the Node surface has no such option (reason in the row's comment);
    /// `None` = the camelCase of the field name.
    pub node: Option<&'static str>,
}

const fn row(name: &'static str) -> OptionInfo {
    OptionInfo {
        name,
        aliases: &[],
        cli: None,
        python: None,
        node: None,
    }
}

/// The per-surface spelling table — see [`OptionInfo`].
pub const OPTIONS: &[OptionInfo] = &[
    // Python picks Markdown strictness at export time (`export_to_markdown`
    // reads `document.strict_markdown`), so the converter takes no kwarg.
    OptionInfo {
        python: Some(""),
        ..row("strict")
    },
    row("compact_tables"),
    // Python: `export_to_markdown(page_break_placeholder=…)`, docling's own
    // signature, not a converter kwarg.
    OptionInfo {
        aliases: &["md_page_break_placeholder"],
        python: Some(""),
        ..row("page_break_placeholder")
    },
    row("fetch_images"),
    row("list_attachments"),
    row("skip_empty_cells"),
    row("ebcdic_layout"),
    row("encoding"),
    // The npm addon is not built with the `web-browser` feature, so the
    // option would only ever error there.
    OptionInfo {
        node: Some(""),
        ..row("use_web_browser")
    },
    row("xbrl_taxonomy"),
    row("asr_model"),
    row("asr_lang"),
    row("video_frames"),
    // Python takes docling's `page_range=(first, last)` tuple.
    OptionInfo {
        python: Some("page_range"),
        ..row("pages")
    },
    row("document_timeout"),
    // Python keeps docling's positive spellings: `do_ocr`,
    // `do_table_structure` — and the docling.rs-only `text_layer_only`.
    // `no_ocr` is docling's `--no-ocr` since 2.0 (#611); `skip_ocr` is its
    // pre-2.0 name, still read (the CLI's `--skip-ocr` too) — Python has
    // docling's `do_ocr` for both.
    row("text_layer_only"),
    OptionInfo {
        python: Some("do_ocr"),
        ..row("no_ocr")
    },
    OptionInfo {
        python: Some(""),
        ..row("skip_ocr")
    },
    // One password for PDFs and Office documents (#625); docling's
    // PDF-only `pdf_password` stays readable on every surface.
    OptionInfo {
        aliases: &["pdf_password"],
        ..row("password")
    },
    row("force_full_page_ocr"),
    OptionInfo {
        python: Some("do_table_structure"),
        ..row("no_table_former")
    },
    row("no_text_panels"),
    row("heading_hierarchy"),
    row("ocr_engine"),
    row("ocr_lang"),
    row("ocr_mode"),
    row("ocr_scale"),
    row("images_scale"),
    OptionInfo {
        aliases: &["generate_page_images"],
        python: Some("generate_page_images"),
        ..row("page_images")
    },
    OptionInfo {
        cli: Some("--enrich-picture-classes"),
        ..row("do_picture_classification")
    },
    OptionInfo {
        cli: Some("--enrich-code"),
        ..row("do_code_enrichment")
    },
    OptionInfo {
        cli: Some("--enrich-formula"),
        ..row("do_formula_enrichment")
    },
    OptionInfo {
        cli: Some("--picture-ocr"),
        ..row("do_picture_ocr")
    },
    row("picture_ocr_classes"),
    row("picture_ocr_min_side"),
    OptionInfo {
        cli: Some("--no-picture-images"),
        ..row("keep_picture_images")
    },
    row("pipeline"),
    row("vlm_endpoint"),
    row("vlm_model"),
    row("vlm_api_key"),
    row("vlm_prompt"),
    row("vlm_max_tokens"),
];

/// The CLI flag for a [`ConvertOptions`] field: its [`OPTIONS`] row's `cli`,
/// else `--` + the name with `_` → `-`.
pub fn cli_flag(field: &str) -> String {
    OPTIONS
        .iter()
        .find(|o| o.name == field)
        .and_then(|o| o.cli)
        .map(str::to_string)
        .unwrap_or_else(|| format!("--{}", field.replace('_', "-")))
}

/// `over` on top of `base`: every field `over` sets (serializes non-null)
/// replaces `base`'s, every one it leaves unset keeps `base`'s — for any
/// all-`Option` options struct, which is why docling-serve's request struct
/// (this one plus its output fields) merges its query / body / form layers
/// through it without a per-field list.
pub fn merge_options<T: Serialize + DeserializeOwned + Default>(over: T, base: T) -> T {
    let mut merged = serde_json::to_value(base).unwrap_or(serde_json::Value::Null);
    let over = serde_json::to_value(over).unwrap_or(serde_json::Value::Null);
    if let (Some(into), Some(from)) = (merged.as_object_mut(), over.as_object()) {
        for (key, value) in from {
            if !value.is_null() {
                into.insert(key.clone(), value.clone());
            }
        }
    }
    // A struct that serialized to an object deserializes from it; the
    // fallback only guards a `T` that is not an object at all.
    serde_json::from_value(merged).unwrap_or_default()
}

impl ConvertOptions {
    /// The wire names of every field, from the struct itself (an unset
    /// option serializes as `null`), so a new field is listed without a
    /// hand-kept list — the inventory test and the unknown-key checks of the
    /// JSON surfaces read it.
    pub fn field_names() -> Vec<String> {
        match serde_json::to_value(Self::default()) {
            Ok(serde_json::Value::Object(map)) => map.keys().cloned().collect(),
            _ => Vec::new(),
        }
    }

    /// The keys among `keys` that are neither a field, one of its aliases,
    /// nor in `also_known` — what a JSON surface that rejects typos (the C
    /// ABI, the wasm module) reports.
    pub fn unknown_keys<'a>(
        keys: impl IntoIterator<Item = &'a str>,
        also_known: &[&str],
    ) -> Vec<String> {
        let fields = Self::field_names();
        keys.into_iter()
            .filter(|k| {
                !fields.iter().any(|f| f == k)
                    && !also_known.contains(k)
                    && !OPTIONS.iter().any(|o| o.aliases.contains(k))
            })
            .map(str::to_string)
            .collect()
    }

    /// `self` on top of `base` — see [`merge_options`].
    pub fn merge_over(self, base: ConvertOptions) -> ConvertOptions {
        merge_options(self, base)
    }

    /// Check every option that can be checked without the input document —
    /// the one set of rules all surfaces apply, before any work starts:
    /// `pages` spelling, a positive `document_timeout`, a positive
    /// `ocr_scale`, `images_scale` in 0.1–4.0, a known `pipeline`, a positive
    /// `vlm_max_tokens`, and (with the `pdf` feature, which has the engine
    /// tables) a known `ocr_engine` / `ocr_mode`, an `ocr_lang` the
    /// selected engine reads and `picture_ocr_classes` the classifier
    /// predicts. `asr_lang`, `encoding` and `ebcdic_layout` are
    /// checked against the model / codec / copybook when the conversion runs.
    pub fn validate(&self) -> Result<(), OptionsError> {
        self.page_range()?;
        self.document_timeout()?;
        if let Some(s) = self.ocr_scale {
            if !(s.is_finite() && s > 0.0) {
                return Err(OptionsError::new(
                    "ocr_scale",
                    format!("ocr_scale must be a positive number, got {s}"),
                ));
            }
        }
        if let Some(s) = self.images_scale {
            if !(0.1..=4.0).contains(&s) {
                return Err(OptionsError::new(
                    "images_scale",
                    format!("images_scale must be a number in 0.1-4.0, got {s}"),
                ));
            }
        }
        self.pipeline()?;
        if self.vlm_max_tokens == Some(0) {
            return Err(OptionsError::new(
                "vlm_max_tokens",
                "vlm_max_tokens must be a positive integer, got 0",
            ));
        }
        #[cfg(feature = "pdf")]
        {
            self.ocr_engine()?;
            self.ocr_mode()?;
            self.ocr_lang()?;
        }
        self.picture_ocr_classes()?;
        Ok(())
    }

    /// The parsed `picture_ocr_classes` (#645): the comma-separated labels,
    /// trimmed and lower-cased, empty when unset. With the `pdf` feature
    /// (which has the classifier's label table) a label the
    /// DocumentFigureClassifier never predicts is rejected — a typo would
    /// otherwise silently skip every picture.
    pub fn picture_ocr_classes(&self) -> Result<Vec<String>, OptionsError> {
        let labels: Vec<String> = self
            .picture_ocr_classes
            .as_deref()
            .unwrap_or("")
            .split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        #[cfg(feature = "pdf")]
        if let Some(bad) = labels
            .iter()
            .find(|l| !docling_pdf::enrich::PICTURE_CLASSES.contains(&l.as_str()))
        {
            return Err(OptionsError::new(
                "picture_ocr_classes",
                format!(
                    "picture_ocr_classes: {bad:?} is not a DocumentFigureClassifier label ({})",
                    docling_pdf::enrich::PICTURE_CLASSES.join(", ")
                ),
            ));
        }
        Ok(labels)
    }

    /// Validate, then set every given option on `base` — unset ones leave the
    /// base's values (so a surface may pre-configure defaults of its own,
    /// e.g. docling-serve's `--strict`). [`DocumentConverter::from_options`]
    /// is this on a fresh converter.
    pub fn apply(&self, mut c: DocumentConverter) -> Result<DocumentConverter, OptionsError> {
        self.validate()?;
        if let Some(v) = self.strict {
            c = c.strict(v);
        }
        if let Some(v) = self.compact_tables {
            c = c.compact_tables(v);
        }
        if self.page_break_placeholder.is_some() {
            c = c.page_break_placeholder(self.page_break_placeholder.clone());
        }
        if let Some(v) = self.fetch_images {
            c = c.fetch_images(v);
        }
        if let Some(v) = self.list_attachments {
            c = c.list_attachments(v);
        }
        if let Some(v) = self.skip_empty_cells {
            c = c.skip_empty_cells(v);
        }
        if self.ebcdic_layout.is_some() {
            c = c.ebcdic_layout_opt(self.ebcdic_layout.clone());
        }
        if self.encoding.is_some() {
            c = c.encoding(self.encoding.clone());
        }
        if let Some(v) = self.use_web_browser {
            c = c.use_web_browser(v);
        }
        if let Some(dir) = &self.xbrl_taxonomy {
            c = c.xbrl_taxonomy(dir);
        }
        if self.asr_model.is_some() {
            c = c.asr_model(self.asr_model.clone());
        }
        if self.asr_lang.is_some() {
            c = c.asr_lang(self.asr_lang.clone());
        }
        if let Some(n) = self.video_frames {
            c = c.video_frames(n);
        }
        if let Some((first, last)) = self.page_range()? {
            c = c.page_range(first, last);
        }
        if self.document_timeout.is_some() {
            c = c.document_timeout(self.document_timeout()?);
        }
        if let Some(v) = self.text_layer_only {
            c = c.text_layer_only(v);
        }
        if self.no_ocr.is_some() || self.skip_ocr.is_some() {
            c = c.no_ocr(self.ocr_disabled());
        }
        if self.password.is_some() {
            c = c.password(self.password.clone());
        }
        if let Some(v) = self.force_full_page_ocr {
            c = c.force_full_page_ocr(v);
        }
        if let Some(v) = self.no_table_former {
            c = c.no_table_former(v);
        }
        if let Some(v) = self.no_text_panels {
            c = c.no_text_panels(v);
        }
        if let Some(v) = self.heading_hierarchy {
            c = c.heading_hierarchy(v);
        }
        if let Some(v) = &self.ocr_engine {
            c = c.ocr_engine(v.clone());
        }
        if let Some(v) = &self.ocr_lang {
            c = c.ocr_lang(v.clone());
        }
        if let Some(v) = &self.ocr_mode {
            c = c.ocr_mode(v.clone());
        }
        if let Some(s) = self.ocr_scale {
            c = c.ocr_scale(s);
        }
        if let Some(s) = self.images_scale {
            c = c.images_scale(s);
        }
        if let Some(v) = self.page_images {
            c = c.generate_page_images(v);
        }
        if let Some(v) = self.do_picture_classification {
            c = c.do_picture_classification(v);
        }
        if let Some(v) = self.do_code_enrichment {
            c = c.do_code_enrichment(v);
        }
        if let Some(v) = self.do_formula_enrichment {
            c = c.do_formula_enrichment(v);
        }
        if let Some(v) = self.do_picture_ocr {
            c = c.do_picture_ocr(v);
        }
        if self.picture_ocr_classes.is_some() {
            c = c.picture_ocr_classes(self.picture_ocr_classes()?);
        }
        if let Some(v) = self.picture_ocr_min_side {
            c = c.picture_ocr_min_side(v);
        }
        if let Some(v) = self.keep_picture_images {
            c = c.keep_picture_images(v);
        }
        Ok(c)
    }

    // --- typed readers, for the surfaces that drive a warm `Pipeline`
    //     themselves and need the parsed values rather than the builder -----

    /// `pages` parsed ([`crate::parse_page_range`]); `None` when unset.
    pub fn page_range(&self) -> Result<Option<(usize, usize)>, OptionsError> {
        self.pages
            .as_deref()
            .map(|p| {
                crate::parse_page_range(p)
                    .map_err(|e| OptionsError::new("pages", format!("pages: {e}")))
            })
            .transpose()
    }

    /// `document_timeout` as a duration; `None` when unset.
    pub fn document_timeout(&self) -> Result<Option<std::time::Duration>, OptionsError> {
        match self.document_timeout {
            Some(s) if s.is_finite() && s > 0.0 => Ok(Some(std::time::Duration::from_secs_f64(s))),
            Some(s) => Err(OptionsError::new(
                "document_timeout",
                format!("document_timeout must be a positive number of seconds, got {s}"),
            )),
            None => Ok(None),
        }
    }

    /// The pipeline the options select.
    pub fn pipeline(&self) -> Result<PipelineKind, OptionsError> {
        match self.pipeline.as_deref().map(str::trim) {
            None | Some("standard") => Ok(PipelineKind::Standard),
            Some("vlm") => Ok(PipelineKind::Vlm),
            Some(other) => Err(OptionsError::new(
                "pipeline",
                format!("unknown pipeline {other:?} (expected: standard, vlm)"),
            )),
        }
    }

    /// The enrichment passes (#423) — unset and `false` both mean off.
    pub fn enrichments(&self) -> crate::EnrichmentOptions {
        crate::EnrichmentOptions {
            picture_classification: self.do_picture_classification.unwrap_or(false),
            code: self.do_code_enrichment.unwrap_or(false),
            formula: self.do_formula_enrichment.unwrap_or(false),
        }
    }

    /// `ocr_engine` parsed; `None` when unset (the process default applies).
    #[cfg(feature = "pdf")]
    pub fn ocr_engine(&self) -> Result<Option<crate::OcrEngine>, OptionsError> {
        self.ocr_engine
            .as_deref()
            .map(|v| {
                crate::OcrEngine::parse(v).ok_or_else(|| {
                    OptionsError::new(
                        "ocr_engine",
                        format!("ocr_engine {v:?} is not {}", crate::OcrEngine::ACCEPTED),
                    )
                })
            })
            .transpose()
    }

    /// The engine `ocr_lang` is read against: `ocr_engine`, else the process
    /// default (`DOCLING_RS_OCR_ENGINE`, else PP-OCR).
    #[cfg(feature = "pdf")]
    pub fn effective_ocr_engine(&self) -> Result<crate::OcrEngine, OptionsError> {
        Ok(self
            .ocr_engine()?
            .unwrap_or_else(crate::OcrEngine::from_env))
    }

    /// `ocr_lang` validated against the engine (#460) and, under PP-OCR, the
    /// recognition model it selects; `None` when unset or under Tesseract
    /// (whose language list is [`Self::tesseract_lang`]).
    #[cfg(feature = "pdf")]
    pub fn ocr_lang(&self) -> Result<Option<crate::OcrLang>, OptionsError> {
        let Some(v) = self.ocr_lang.as_deref() else {
            return Ok(None);
        };
        let engine = self.effective_ocr_engine()?;
        engine.validate_lang(v).map_err(|e| {
            OptionsError::new(
                "ocr_lang",
                if e.contains("ocr_lang") {
                    e
                } else {
                    format!("ocr_lang: {e}")
                },
            )
        })?;
        Ok(match engine {
            crate::OcrEngine::PpOcr => crate::OcrLang::parse(v),
            crate::OcrEngine::Tesseract => None,
        })
    }

    /// Tesseract's `-l` argument from `ocr_lang` (#460); `None` under PP-OCR
    /// or without a language.
    #[cfg(feature = "pdf")]
    pub fn tesseract_lang(&self) -> Result<Option<String>, OptionsError> {
        let Some(v) = self.ocr_lang.as_deref() else {
            return Ok(None);
        };
        match self.effective_ocr_engine()? {
            crate::OcrEngine::Tesseract => crate::tesseract_lang_arg(v)
                .map(Some)
                .map_err(|e| OptionsError::new("ocr_lang", e)),
            crate::OcrEngine::PpOcr => Ok(None),
        }
    }

    /// Whether OCR is off: `no_ocr` or its pre-2.0 name `skip_ocr` (#611).
    /// What a surface that builds the PDF pipeline itself reads.
    pub fn ocr_disabled(&self) -> bool {
        self.no_ocr.unwrap_or(false) || self.skip_ocr.unwrap_or(false)
    }

    /// `ocr_mode` parsed (#254); `None` when unset.
    #[cfg(feature = "pdf")]
    pub fn ocr_mode(&self) -> Result<Option<crate::OcrMode>, OptionsError> {
        self.ocr_mode
            .as_deref()
            .map(|v| {
                crate::OcrMode::parse(v).ok_or_else(|| {
                    OptionsError::new(
                        "ocr_mode",
                        format!(
                            "ocr_mode {v:?} is not \
                             default|full_page|layout_regions|pdf_aware_layout_regions"
                        ),
                    )
                })
            })
            .transpose()
    }

    /// Picture-crop scale and page images for the pipeline (#519/#520).
    #[cfg(feature = "pdf")]
    pub fn image_output(&self) -> crate::ImageOutput {
        crate::ImageOutput {
            scale: self.images_scale,
            page_images: self.page_images.unwrap_or(false),
        }
    }

    /// The remote-VLM configuration (#77) when `pipeline` is `vlm`, resolved
    /// against the `DOCLING_RS_VLM_*` environment the way every surface did
    /// on its own before: endpoint and model from the options else the env
    /// (blank counts as unset), the prompt and API key likewise, `pages`
    /// composing like it does with the ML pipeline. `None` under the standard
    /// pipeline — the `vlm_*` options are then ignored, not rejected (the
    /// bindings' long-standing contract).
    #[cfg(feature = "vlm")]
    pub fn vlm_options(&self) -> Result<Option<crate::vlm::VlmOptions>, OptionsError> {
        if self.pipeline()? != PipelineKind::Vlm {
            return Ok(None);
        }
        let set = |s: &Option<String>| s.clone().filter(|v| !v.trim().is_empty());
        let mut v = crate::vlm::VlmOptions::resolve(set(&self.vlm_endpoint), set(&self.vlm_model))
            .map_err(|e| {
                // The library's message names the CLI flags; the wire spelling
                // is this surface's (the CLI swaps its flags back in).
                let msg = match e {
                    crate::ConversionError::Parse(m) => m,
                    other => other.to_string(),
                }
                .replace("pass --vlm-endpoint", "set vlm_endpoint")
                .replace("pass --vlm-model", "set vlm_model");
                let field = if msg.contains("vlm_model") {
                    "vlm_model"
                } else {
                    "vlm_endpoint"
                };
                OptionsError::new(field, msg)
            })?;
        if let Some(p) = set(&self.vlm_prompt) {
            v.prompt = Some(p);
        }
        if let Some(k) = set(&self.vlm_api_key) {
            v.api_key = Some(k);
        }
        match self.vlm_max_tokens {
            // 0 would have every page come back empty and surface as a model
            // error — a message pointing at the model, not at the option.
            Some(0) => {
                return Err(OptionsError::new(
                    "vlm_max_tokens",
                    "vlm_max_tokens must be a positive integer, got 0",
                ))
            }
            Some(n) => v.max_tokens = n,
            None => {}
        }
        v.page_range = self.page_range()?;
        Ok(Some(v))
    }
}

impl DocumentConverter {
    /// A converter configured from [`ConvertOptions`] — validated and applied
    /// onto the defaults ([`ConvertOptions::apply`] on [`Self::new`]).
    pub fn from_options(options: &ConvertOptions) -> Result<Self, OptionsError> {
        options.apply(Self::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spelling table and the struct describe the same fields, both ways
    /// — the guard that makes "add a field" also mean "add its row".
    #[test]
    fn options_table_matches_the_struct() {
        let fields = ConvertOptions::field_names();
        for o in OPTIONS {
            assert!(
                fields.iter().any(|f| f == o.name),
                "no field for row {}",
                o.name
            );
            for a in o.aliases {
                assert!(!fields.iter().any(|f| f == a), "alias {a} is also a field");
            }
        }
        for f in &fields {
            assert!(
                OPTIONS.iter().any(|o| o.name == f),
                "no OPTIONS row for field {f}"
            );
        }
        assert_eq!(cli_flag("ocr_scale"), "--ocr-scale");
        assert_eq!(cli_flag("do_code_enrichment"), "--enrich-code");
    }

    #[test]
    fn aliases_deserialize_and_unknown_keys_are_reported() {
        let o: ConvertOptions = serde_json::from_str(
            r#"{"md_page_break_placeholder": "<!-- p -->", "generate_page_images": true}"#,
        )
        .unwrap();
        assert_eq!(o.page_break_placeholder.as_deref(), Some("<!-- p -->"));
        assert_eq!(o.page_images, Some(true));
        assert_eq!(
            ConvertOptions::unknown_keys(
                ["to", "strict", "md_page_break_placeholder", "strictness"],
                &["to"]
            ),
            vec!["strictness".to_string()]
        );
    }

    #[test]
    fn merge_prefers_the_overlay_and_keeps_the_rest() {
        let base = ConvertOptions {
            strict: Some(true),
            ocr_scale: Some(2.0),
            pages: Some("1-3".into()),
            ..Default::default()
        };
        let over = ConvertOptions {
            strict: Some(false),
            video_frames: Some(0),
            ..Default::default()
        };
        let merged = over.merge_over(base);
        assert_eq!(merged.strict, Some(false));
        assert_eq!(merged.ocr_scale, Some(2.0));
        assert_eq!(merged.pages.as_deref(), Some("1-3"));
        assert_eq!(merged.video_frames, Some(0));
    }

    #[test]
    fn validation_rules_and_cli_spelling() {
        let bad = |o: ConvertOptions| o.validate().unwrap_err();
        let e = bad(ConvertOptions {
            ocr_scale: Some(0.0),
            ..Default::default()
        });
        assert_eq!(e.field, "ocr_scale");
        assert_eq!(e.to_string(), "ocr_scale must be a positive number, got 0");
        assert_eq!(
            e.cli_message(),
            "--ocr-scale must be a positive number, got 0"
        );

        let e = bad(ConvertOptions {
            images_scale: Some(9.0),
            ..Default::default()
        });
        assert_eq!(e.field, "images_scale");
        let e = bad(ConvertOptions {
            document_timeout: Some(-1.0),
            ..Default::default()
        });
        assert!(e.message.contains("document_timeout must be a positive"));
        let e = bad(ConvertOptions {
            pages: Some("5-3".into()),
            ..Default::default()
        });
        assert_eq!(e.field, "pages");
        assert!(
            e.cli_message().starts_with("--pages: "),
            "{}",
            e.cli_message()
        );
        let e = bad(ConvertOptions {
            pipeline: Some("magic".into()),
            ..Default::default()
        });
        assert!(e.message.contains("unknown pipeline"), "{e}");
        let e = bad(ConvertOptions {
            vlm_max_tokens: Some(0),
            ..Default::default()
        });
        assert_eq!(
            e.cli_message(),
            "--vlm-max-tokens must be a positive integer, got 0"
        );
        assert!(ConvertOptions::default().validate().is_ok());
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn ocr_options_are_validated_against_the_engine() {
        let e = ConvertOptions {
            ocr_engine: Some("easyocr".into()),
            ..Default::default()
        }
        .validate()
        .unwrap_err();
        assert_eq!(e.field, "ocr_engine");
        assert!(
            e.cli_message().starts_with("--ocr-engine \"easyocr\""),
            "{}",
            e.cli_message()
        );
        // `deu` is a language to Tesseract only.
        let e = ConvertOptions {
            ocr_engine: Some("ppocr".into()),
            ocr_lang: Some("deu".into()),
            ..Default::default()
        }
        .validate()
        .unwrap_err();
        assert_eq!(e.field, "ocr_lang");
        assert!(e.message.contains("ocr_lang"), "{e}");
        let tess = ConvertOptions {
            ocr_engine: Some("tesseract".into()),
            ocr_lang: Some("deu+fra".into()),
            ..Default::default()
        };
        assert!(tess.validate().is_ok());
        assert_eq!(tess.ocr_lang().unwrap(), None);
        assert_eq!(tess.tesseract_lang().unwrap().as_deref(), Some("deu+fra"));
        let e = ConvertOptions {
            ocr_mode: Some("sideways".into()),
            ..Default::default()
        }
        .validate()
        .unwrap_err();
        assert_eq!(e.field, "ocr_mode");
    }

    #[test]
    fn apply_sets_only_what_is_given() {
        // `strict` and the page-break text ride on the finished document, so
        // they are observable without a conversion.
        let base = DocumentConverter::new().strict(true);
        let mut doc = docling_core::DoclingDocument::new("t");
        ConvertOptions::default()
            .apply(base.clone())
            .unwrap()
            .finish_document(&mut doc);
        assert!(doc.strict_markdown, "unset keeps the base's strict");
        let mut doc = docling_core::DoclingDocument::new("t");
        ConvertOptions {
            strict: Some(false),
            page_break_placeholder: Some("<!-- p -->".into()),
            ..Default::default()
        }
        .apply(base)
        .unwrap()
        .finish_document(&mut doc);
        assert!(!doc.strict_markdown);
        assert_eq!(doc.page_break_placeholder.as_deref(), Some("<!-- p -->"));
        // A rejected option never yields a converter.
        assert!(DocumentConverter::from_options(&ConvertOptions {
            pages: Some("0-2".into()),
            ..Default::default()
        })
        .is_err());
    }

    #[cfg(feature = "vlm")]
    #[test]
    fn vlm_options_follow_the_pipeline_switch() {
        // Standard: stray vlm_* options are ignored, nothing resolves.
        let stray = ConvertOptions {
            vlm_endpoint: Some("http://127.0.0.1:9/v1".into()),
            ..Default::default()
        };
        assert!(matches!(stray.vlm_options(), Ok(None)));
        let full = ConvertOptions {
            pipeline: Some("vlm".into()),
            vlm_endpoint: Some("http://example.com/v1".into()),
            vlm_model: Some("m".into()),
            vlm_api_key: Some("sk-test".into()),
            vlm_prompt: Some("Read the page.".into()),
            vlm_max_tokens: Some(512),
            pages: Some("2-5".into()),
            ..Default::default()
        };
        let v = full.vlm_options().unwrap().expect("resolves");
        assert_eq!(v.endpoint, "http://example.com/v1");
        assert_eq!(v.model, "m");
        assert_eq!(v.api_key.as_deref(), Some("sk-test"));
        assert_eq!(v.prompt.as_deref(), Some("Read the page."));
        assert_eq!(v.max_tokens, 512);
        assert_eq!(v.page_range, Some((2, 5)));
        let e = ConvertOptions {
            vlm_max_tokens: Some(0),
            ..full.clone()
        }
        .vlm_options()
        .unwrap_err();
        assert_eq!(e.field, "vlm_max_tokens");
        // No model anywhere: the message names the wire option, the CLI the
        // flag. (`DOCLING_RS_VLM_MODEL` unset in the test environment.)
        if std::env::var_os("DOCLING_RS_VLM_MODEL").is_none() {
            let e = ConvertOptions {
                vlm_model: None,
                ..full
            }
            .vlm_options()
            .unwrap_err();
            assert_eq!(e.field, "vlm_model");
            assert!(e.message.contains("set vlm_model"), "{e}");
            assert!(
                e.cli_message().contains("--vlm-model"),
                "{}",
                e.cli_message()
            );
            assert!(e.message.contains("DOCLING_RS_VLM_MODEL"), "{e}");
        }
    }

    /// #611: `no_ocr` is docling's `do_ocr=False` and `skip_ocr` its pre-2.0
    /// name — either one turns OCR off, and a body carrying both (a pre-2.0
    /// client sending both switches) parses instead of failing on a
    /// duplicate field; `text_layer_only` and `password` — under docling's
    /// `pdf_password` too (#625) — are read.
    #[test]
    fn ocr_spellings_and_password_parse() {
        let both: ConvertOptions = serde_json::from_str(
            r#"{"no_ocr": false, "skip_ocr": true, "text_layer_only": true,
                "pdf_password": "1234"}"#,
        )
        .expect("both spellings parse");
        assert!(both.ocr_disabled());
        assert_eq!(both.text_layer_only, Some(true));
        assert_eq!(both.password.as_deref(), Some("1234"));
        let neutral: ConvertOptions = serde_json::from_str(r#"{"password": "x"}"#).unwrap();
        assert_eq!(neutral.password.as_deref(), Some("x"));
        let new: ConvertOptions = serde_json::from_str(r#"{"no_ocr": true}"#).unwrap();
        assert!(new.ocr_disabled());
        assert!(!ConvertOptions::default().ocr_disabled());
        // Merged like every option: a later layer's spelling adds to the base.
        let merged = new.merge_over(both);
        assert!(merged.ocr_disabled());
        assert_eq!(merged.password.as_deref(), Some("1234"));
    }
}
