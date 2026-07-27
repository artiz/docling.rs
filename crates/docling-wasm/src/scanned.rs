//! Browser scanned-document pipeline — stage 2 of #157 (the *lite profile*):
//! RT-DETR layout detection + PP-OCRv3 recognition, both via ONNX Runtime Web
//! on the JS side; region refinement, region-cropped OCR, geometric table
//! reconstruction and reading-order assembly in Rust — the same code as the
//! native pipeline (`docling_pdf::{layout, ocr_prep, scanned}`). TableFormer
//! and the enrichment models are stage 3; table regions fall back to the
//! geometric reconstruction the native `--no-table-former` flag uses. Picture
//! regions are cropped out of the page bitmap, like the native pipeline, so
//! `images = "embedded"` inlines real figure bytes.
//!
//! Pages arrive as raw RGBA bitmaps: for a scanned PDF the host page renders
//! them with pdf.js (`page.getViewport({scale: 2})` — 2 px per PDF point,
//! matching the native pipeline's `RENDER_SCALE`); a standalone image is its
//! own page at scale 1, exactly like the native image path.
//!
//! ```js
//! const conv = new ScannedConverter(dictText);
//! for (const bitmap of pages) {
//!   await conv.add_page(bitmap.data, bitmap.width, bitmap.height, 2.0, layout, rec);
//! }
//! const markdown = conv.finish("scan.pdf", "md", "embedded");
//! ```
//! (`www/index.html` is the complete wiring.)

use docling_pdf::assemble::{geometric_table_is_reliable, reconstruct_table};
use docling_pdf::layout::{decode_layout, layout_input, SIDE};
use docling_pdf::ocr_prep::{
    batch_input, decode_row, dict_chars, normalize_polarity, prep_region_lines, prep_table_words,
    width_batches, PrepLine, REC_HEIGHT,
};
use docling_pdf::pdfium_backend::{PdfPage, TextCell};
use docling_pdf::scanned::{assemble_page_with_tables, finish_document, refine_regions};
use image::RgbImage;
use wasm_bindgen::prelude::*;

use crate::ocr::{tensor_parts, RecSession};
use crate::tableformer::TfSession;

#[wasm_bindgen]
extern "C" {
    /// The JS-side layout session: a wrapper around an `ort.InferenceSession`
    /// over the RT-DETR layout model exposing `run(data)` — feed the
    /// `(1, 3, 640, 640)` CHW float buffer, resolve to
    /// `{ logits: {data, dims: [1, q, c]}, boxes: {data, dims: [1, q, 4]} }`.
    pub type LayoutSession;

    #[wasm_bindgen(method, catch)]
    pub async fn run(this: &LayoutSession, data: js_sys::Float32Array) -> Result<JsValue, JsValue>;
}

/// Multi-page scanned-document converter (lite profile). Feed pages in
/// order, then [`finish`](Self::finish) — cross-page paragraph continuations
/// merge exactly like the native pipeline.
#[wasm_bindgen]
pub struct ScannedConverter {
    chars: Vec<String>,
    pages: Vec<docling_pdf::scanned::AssembledPage>,
}

#[wasm_bindgen]
impl ScannedConverter {
    /// `dict` is the recognition dictionary text (`en_dict.txt` for the
    /// default English model).
    #[wasm_bindgen(constructor)]
    pub fn new(dict: &str) -> Self {
        Self {
            chars: dict_chars(dict),
            pages: Vec::new(),
        }
    }

    /// Convert one page (lite profile — geometric tables): `rgba` is the
    /// rendered bitmap (canvas ImageData), `scale` its pixels-per-PDF-point
    /// (2.0 for pdf.js `{scale: 2}`; 1.0 for a standalone image).
    pub async fn add_page(
        &mut self,
        rgba: &[u8],
        px_w: u32,
        px_h: u32,
        scale: f32,
        layout: &LayoutSession,
        rec: &RecSession,
    ) -> Result<(), JsError> {
        self.add_page_impl(rgba, px_w, px_h, scale, layout, rec, None)
            .await
    }

    /// Convert one page with TableFormer (#157 stage 3): table regions get the
    /// ONNX table-structure model + docling's cell matcher instead of the
    /// geometric reconstruction. `tf` is the JS-side session over the encoder /
    /// decoder / bbox graphs.
    #[wasm_bindgen(js_name = addPageTf)]
    #[allow(clippy::too_many_arguments)] // wasm-bindgen entry: page bitmap + 3 sessions
    pub async fn add_page_tf(
        &mut self,
        rgba: &[u8],
        px_w: u32,
        px_h: u32,
        scale: f32,
        layout: &LayoutSession,
        rec: &RecSession,
        tf: &TfSession,
    ) -> Result<(), JsError> {
        self.add_page_impl(rgba, px_w, px_h, scale, layout, rec, Some(tf))
            .await
    }
}

impl ScannedConverter {
    /// Recognize a set of prepared line/word crops: width-batch them and run
    /// each batch through the JS recognition session, greedy-CTC-decoding the
    /// probabilities. Returns one string per input crop, in order.
    async fn ocr_lines(
        &self,
        rec: &RecSession,
        lines: &[PrepLine],
    ) -> Result<Vec<String>, JsError> {
        let mut texts = vec![String::new(); lines.len()];
        for (w, chunk) in width_batches(lines) {
            let data = batch_input(w, &chunk, lines);
            let out = rec
                .run(
                    chunk.len() as u32,
                    REC_HEIGHT,
                    w as u32,
                    js_sys::Float32Array::from(data.as_slice()),
                )
                .await
                .map_err(|e| JsError::new(&format!("rec session.run: {e:?}")))?;
            let (probs, t_len, nc) = tensor_parts(&out)?;
            if probs.len() < chunk.len() * t_len * nc {
                return Err(JsError::new("rec session.run returned a short tensor"));
            }
            for (i, &ix) in chunk.iter().enumerate() {
                texts[ix] = decode_row(
                    &self.chars,
                    &probs[i * t_len * nc..(i + 1) * t_len * nc],
                    nc,
                );
            }
        }
        Ok(texts)
    }

    #[allow(clippy::too_many_arguments)]
    async fn add_page_impl(
        &mut self,
        rgba: &[u8],
        px_w: u32,
        px_h: u32,
        scale: f32,
        layout: &LayoutSession,
        rec: &RecSession,
        tf: Option<&TfSession>,
    ) -> Result<(), JsError> {
        if rgba.len() != (px_w as usize) * (px_h as usize) * 4 {
            return Err(JsError::new("rgba buffer size does not match dimensions"));
        }
        let mut img = RgbImage::new(px_w, px_h);
        for (i, px) in img.pixels_mut().enumerate() {
            px.0 = [rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2]];
        }
        // Dark-mode screenshots invert scan polarity; normalize before both
        // layout and OCR (each assumes dark ink on light paper).
        let img = normalize_polarity(img);
        let (page_w, page_h) = (px_w as f32 / scale, px_h as f32 / scale);

        // Layout: Rust preprocessing → JS inference → Rust decoding.
        let input = layout_input(&img);
        let out = layout
            .run(js_sys::Float32Array::from(input.as_slice()))
            .await
            .map_err(|e| JsError::new(&format!("layout session.run: {e:?}")))?;
        let get = |k: &str| {
            js_sys::Reflect::get(&out, &JsValue::from_str(k))
                .map_err(|_| JsError::new(&format!("layout result has no `{k}`")))
        };
        let (logits, q, c) = tensor_parts(&get("logits")?)?;
        let (boxes, bq, four) = tensor_parts(&get("boxes")?)?;
        if bq != q || four != 4 {
            return Err(JsError::new("layout boxes dims must be [1, q, 4]"));
        }
        let regions = decode_layout(&logits, &boxes, q, c, page_w, page_h);
        let regions = refine_regions(regions, &[], page_w, page_h);

        // OCR the text regions (same gather/batch/decode as native ocr_page).
        let (bboxes, lines) = prep_region_lines(&img, &regions, scale);
        let texts = self.ocr_lines(rec, &lines).await?;
        let mut cells = Vec::new();
        for ((l, t, r, b), text) in bboxes.into_iter().zip(texts) {
            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            cells.push(TextCell { text, l, t, r, b });
        }

        // Table interiors carry no words yet (prep_region_lines skips non-text
        // labels, and the browser has no pdfium text layer). Recognize their
        // word crops so the cell matcher — geometric or TableFormer — can fill
        // the grid; assemble routes these cells into the table region, not into
        // stray paragraphs.
        let (tbboxes, tlines) = prep_table_words(&img, &regions, scale);
        let ttexts = self.ocr_lines(rec, &tlines).await?;
        for ((l, t, r, b), text) in tbboxes.into_iter().zip(ttexts) {
            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            cells.push(TextCell { text, l, t, r, b });
        }

        // TableFormer (opt-in): resolve each table region's structure through
        // the ONNX graphs + shared matcher; other regions stay `None` (geometric
        // fallback). The lite path passes no session → all geometric.
        let table_rows = if let Some(tf) = tf {
            let mut rows = Vec::with_capacity(regions.len());
            for r in &regions {
                if r.label != "table" {
                    rows.push(None);
                    continue;
                }
                // TableFormer costs seconds per region (the fp32 encoder runs
                // once per table), so spend it only where it buys something:
                // when the free geometric reconstruction already yields a dense,
                // well-formed grid it is what TableFormer would agree with, and
                // `None` tells assemble to keep it.
                let geometric = reconstruct_table(r, &cells);
                if geometric_table_is_reliable(&geometric) {
                    rows.push(None);
                    continue;
                }
                rows.push(
                    crate::tableformer::predict_table_rows(tf, &img, [r.l, r.t, r.r, r.b], &cells)
                        .await,
                );
            }
            rows
        } else {
            Vec::new() // assemble_page_with_tables resizes to all-None
        };

        // Hand the page bitmap over: assemble crops each `picture` region out
        // of it, so the browser pipeline produces the same figure bytes the
        // native one does (`images=embedded` then inlines them).
        let page = PdfPage::from_cells_with_image(page_w, page_h, scale, cells, img);
        self.pages
            .push(assemble_page_with_tables(&page, regions, table_rows));
        Ok(())
    }
}

#[wasm_bindgen]
impl ScannedConverter {
    /// Number of pages converted so far (progress display).
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Assemble the accumulated pages into the final document and render it
    /// as `"md"` (default), `"json"` or `"doclang"` — the same three the
    /// declarative [`crate::convert`] entry point offers. `images` picks how
    /// cropped figures render in Markdown (`"placeholder"` | `"embedded"`),
    /// like [`crate::convert`]. Resets the converter.
    pub fn finish(
        &mut self,
        name: &str,
        to: Option<String>,
        images: Option<String>,
    ) -> Result<String, JsError> {
        let doc = finish_document(name, std::mem::take(&mut self.pages));
        render(&doc, to.as_deref(), images.as_deref())
    }
}

/// Render an assembled document in one of the three output grammars, with the
/// same `images` choice the declarative path offers — picture regions are
/// cropped out of the rendered page, so `embedded` has real bytes to inline.
fn render(
    doc: &docling_core::DoclingDocument,
    to: Option<&str>,
    images: Option<&str>,
) -> Result<String, JsError> {
    match to.unwrap_or("md") {
        "md" | "markdown" => {
            let mode = crate::image_mode(images).map_err(|e| JsError::new(&e))?;
            Ok(doc.export_to_markdown_with_images(mode, "artifacts").0)
        }
        "json" => Ok(doc.export_to_json()),
        "doclang" => Ok(doc.export_to_doclang()),
        other => Err(JsError::new(&format!(
            "unknown output format {other:?} (expected \"md\", \"json\" or \"doclang\")"
        ))),
    }
}

/// One-shot scanned-image conversion through the full lite profile (layout +
/// OCR + assembly) — the browser counterpart of the native image path
/// (a standalone image is its own page at scale 1).
#[wasm_bindgen]
pub async fn convert_scanned_image(
    bytes: &[u8],
    name: &str,
    dict: &str,
    layout: &LayoutSession,
    rec: &RecSession,
    to: Option<String>,
    images: Option<String>,
) -> Result<String, JsError> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| JsError::new(&format!("decode image: {e}")))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    let mut conv = ScannedConverter::new(dict);
    conv.add_page(img.as_raw(), w, h, 1.0, layout, rec).await?;
    conv.finish(name, to, images)
}

// Silence the unused warning for SIDE re-export path (the JS side sizes its
// tensor from the buffer length, but the constant documents the contract).
const _: u32 = SIDE;
