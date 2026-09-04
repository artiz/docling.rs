//! Layout detection via the RT-DETR (`docling-layout-heron`) model exported to
//! ONNX, run with `ort`. A port of docling-ibm-models' `LayoutPredictor`:
//! resize the page image to 640×640 and rescale to `[0,1]` (the heron processor
//! has `do_normalize=false`), run the model, then RT-DETR
//! `post_process_object_detection` (sigmoid → top-k over query×class →
//! center-to-corners boxes scaled to the page).

#[cfg(feature = "ml")]
use image::imageops::FilterType;
#[cfg(feature = "ml")]
use ort::session::Session;
#[cfg(feature = "ml")]
use ort::value::Tensor;

/// The 17 canonical layout classes, indexed by the model's class id
/// (`config.json` `id2label`).
pub const LABELS: [&str; 17] = [
    "caption",
    "footnote",
    "formula",
    "list_item",
    "page_footer",
    "page_header",
    "picture",
    "section_header",
    "table",
    "text",
    "title",
    "document_index",
    "code",
    "checkbox_selected",
    "checkbox_unselected",
    "form",
    "key_value_region",
];

/// One detected region, in page points (top-left origin).
#[derive(Debug, Clone)]
pub struct Region {
    pub label: &'static str,
    pub score: f32,
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
}

/// What a layout inference call receives per page — which resize kernel packs
/// the 640×640 model input depends on it (docling parity, #58-branch):
///
/// docling's layout stage runs on `page.get_image(scale=1.0)` — the
/// point-sized page image (pdfium at 1.5×, PIL-BICUBIC down) — which its
/// RT-DETR processor then stretches to 640×640 with **PIL BILINEAR**
/// (`preprocessor_config.json`: `do_pad: false`, `resample: 2`; no letterbox,
/// no normalize beyond `/255`). [`PageImage`](LayoutSrc::PageImage) is that
/// image and goes through the byte-exact PIL kernel. [`Raw`](LayoutSrc::Raw)
/// is any other bitmap (the browser path's canvas render, METS/TIFF page
/// scans) and keeps the legacy Triangle stretch.
#[cfg(feature = "ocr-prep")]
#[derive(Clone, Copy)]
pub enum LayoutSrc<'a> {
    /// The scale-1.0 page image (`PdfPage::image_layout`), docling-exact.
    PageImage(&'a image::RgbImage),
    /// Any other page bitmap — legacy stretch.
    Raw(&'a image::RgbImage),
}

/// Base confidence threshold (docling-ibm-models `base_threshold`): the raw
/// RT-DETR floor before docling's `LayoutPostprocessor` applies its stricter
/// per-label thresholds ([`label_threshold`]).
const THRESHOLD: f32 = 0.3;
/// RT-DETR's fixed square input side.
pub const SIDE: u32 = 640;

/// Per-label confidence threshold, ported from docling's
/// `LayoutPostprocessor.CONFIDENCE_THRESHOLDS`. The raw predictor keeps every
/// detection above the 0.3 base; the postprocessor then drops a cluster whose
/// score is below its label's threshold. Applying it here (equivalent, since
/// every per-label threshold is ≥ the 0.3 base) keeps low-confidence pictures /
/// tables / list-items out of the assembly, matching docling.
pub fn label_threshold(label: &str) -> f32 {
    match label {
        "section_header"
        | "title"
        | "code"
        | "checkbox_selected"
        | "checkbox_unselected"
        | "form"
        | "key_value_region"
        | "document_index" => 0.45,
        // caption, footnote, formula, list_item, page_footer, page_header,
        // picture, table, text — all 0.5 in docling.
        _ => 0.5,
    }
}

#[cfg(feature = "ml")]
pub struct LayoutModel {
    session: Session,
    /// Set when a multi-page inference fails — e.g. a locally built pre-#73
    /// static graph (fixed batch=1) via `DOCLING_LAYOUT_ONNX` or a stale
    /// `layout_heron_int8.onnx`. Batched calls then fall back to per-page runs
    /// instead of failing the conversion.
    batch_unsupported: bool,
    /// The fp32 graph to escalate a suspicious page to, set only when the
    /// *auto-selected* int8 graph loaded (an explicit `DOCLING_LAYOUT_ONNX` /
    /// `DOCLING_RS_FP32` choice is respected). Int8 confidences sit close
    /// enough to the 0.5 label thresholds that a different CPU's quantized
    /// kernels (AVX-VNNI vs AVX2, CUDA's fallback mix) can flip a whole page's
    /// detections — observed as a bill page whose tables all dissolved into
    /// orphan lines on one machine while converting perfectly on another.
    fp32_path: Option<String>,
    /// Lazily-loaded session over `fp32_path` — most documents never pay for it.
    fp32: Option<Session>,
    /// Intra-op threads, kept for the lazy fp32 load.
    intra: usize,
}

#[cfg(feature = "ml")]
impl LayoutModel {
    /// Load the ONNX model from `DOCLING_LAYOUT_ONNX`. Without the override,
    /// prefers `.models/layout_heron_int8.onnx` when present (the quantized
    /// default; `DOCLING_RS_FP32=1` opts out), else `.models/layout_heron.onnx`.
    pub fn load() -> Result<Self, String> {
        Self::load_with(crate::intra_threads())
    }

    /// Like [`load`](Self::load) but with an explicit intra-op thread count. A
    /// parallel page-worker pool loads its helper models on a single thread each
    /// and gets its speed-up from running pages concurrently instead.
    pub fn load_with(intra: usize) -> Result<Self, String> {
        let path = crate::model_path(
            "DOCLING_LAYOUT_ONNX",
            ".models/layout_heron.onnx",
            ".models/layout_heron_int8.onnx",
        );
        if crate::timing::enabled() {
            eprintln!("docling-pdf: layout model: {path}");
        }
        // Escalation target for the quant-robustness guard: only when the
        // int8 graph was picked automatically and the fp32 one is also there.
        let fp32_path = if docling_core::env::nonempty("DOCLING_LAYOUT_ONNX").is_none() {
            let fp32 = crate::resolve_asset(".models/layout_heron.onnx");
            (path != fp32 && std::path::Path::new(&fp32).exists()).then_some(fp32)
        } else {
            None
        };
        let session = Self::open_session(&path, intra)?;
        Ok(Self {
            session,
            batch_unsupported: false,
            fp32_path,
            fp32: None,
            intra,
        })
    }

    fn open_session(path: &str, intra: usize) -> Result<Session, String> {
        // The layout model is the pipeline's first hard model dependency; a
        // missing file here almost always means the models were never
        // downloaded (`cargo install` ships none) — say what to do.
        if !std::path::Path::new(path).exists() {
            return Err(format!(
                "layout: model not found at {path} — PDF/image conversion needs \
                 the ONNX models: fetch them with \
                 scripts/install/download_dependencies.sh from a docling.rs \
                 checkout (https://github.com/docling-project/docling.rs), or \
                 set DOCLING_LAYOUT_ONNX. A digital PDF's embedded text layer \
                 converts without models in no-OCR mode (CLI: --no-ocr)"
            ));
        }
        let mut builder = Session::builder()
            .map_err(|e| format!("layout: builder: {e}"))?
            // Let inference use the available cores (ort otherwise defaults low);
            // a large PDF runs this model once per page.
            .with_intra_threads(intra)
            .map_err(|e| format!("layout: intra_threads: {e}"))?;
        // Per-page mode pins the model's dynamic `batch` axis to 1 (#339):
        // the free dimension blocks ONNX Runtime's channels-last conv
        // transform, so the graph runs NCHW `FusedConv` instead of
        // `NhwcFusedConv` — the issue measured ~1.4× on Apple-silicon CPU
        // for the same weights re-exported static. Overriding the dimension
        // at session creation gets the static graph without a re-export; it
        // also leaves the whole graph static-shaped, which is what the
        // CoreML provider's static-partitions default (#324) wants. Batched
        // mode keeps the axis free — those sessions must accept N pages.
        if crate::pdf_layout_batch() == 1 {
            builder = builder
                .with_dimension_override("batch", 1)
                .map_err(|e| format!("layout: dimension override: {e}"))?;
        }
        docling_onnx::apply(builder)
            .map_err(|e| format!("layout: {e}"))?
            .commit_from_file(path)
            .map_err(|e| format!("layout: load {path}: {e}"))
    }

    /// Re-run one page through the fp32 graph — the escape hatch for a page
    /// whose int8 detections look implausible (see `fp32_path`). `Ok(None)`
    /// when there is nothing to escalate to: fp32 already loaded, an explicit
    /// model override, or no fp32 file on disk.
    pub fn predict_fp32_fallback(
        &mut self,
        img: LayoutSrc<'_>,
        page_w: f32,
        page_h: f32,
    ) -> Result<Option<Vec<Region>>, String> {
        let Some(path) = self.fp32_path.clone() else {
            return Ok(None);
        };
        if self.fp32.is_none() {
            if crate::timing::enabled() {
                eprintln!("docling-pdf: loading fp32 layout fallback: {path}");
            }
            self.fp32 = Some(Self::open_session(&path, self.intra)?);
        }
        let session = self.fp32.as_mut().expect("just loaded");
        Ok(Some(
            Self::run_on(session, &[(img, page_w, page_h)])?
                .pop()
                .expect("one result per input page"),
        ))
    }

    /// Detect layout regions on a page image. `page_w`/`page_h` are the page size
    /// in points; returned boxes are in those coordinates.
    pub fn predict(
        &mut self,
        img: LayoutSrc<'_>,
        page_w: f32,
        page_h: f32,
    ) -> Result<Vec<Region>, String> {
        Ok(self
            .predict_batch(&[(img, page_w, page_h)])?
            .pop()
            .expect("one result per input page"))
    }

    /// Detect layout regions on several page images with **one** inference call
    /// (issue #73). The ONNX export has a dynamic batch dimension, so a worker
    /// can amortize the per-run framework overhead and keep its cores busier on
    /// multi-page documents. Results are per-image, index-aligned with `pages`,
    /// and identical to calling [`predict`](Self::predict) per page.
    pub fn predict_batch(
        &mut self,
        pages: &[(LayoutSrc<'_>, f32, f32)],
    ) -> Result<Vec<Vec<Region>>, String> {
        if pages.len() > 1 && self.batch_unsupported {
            return self.predict_singly(pages);
        }
        match self.run_batch(pages) {
            Err(e) if pages.len() > 1 => {
                // A graph without the dynamic batch dim (pre-#73 export) fails
                // only for batch > 1 — remember and recover per page. Warn once
                // per process, not per worker: every worker owns a LayoutModel
                // over the same graph file, so repeats carry no information.
                static WARNED: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    eprintln!(
                        "docling-pdf: layout model rejected a {}-page batch ({e}); \
                         falling back to per-page inference — re-export with \
                         scripts/install/export_layout.py for batched layout",
                        pages.len()
                    );
                }
                self.batch_unsupported = true;
                self.predict_singly(pages)
            }
            other => other,
        }
    }

    fn predict_singly(
        &mut self,
        pages: &[(LayoutSrc<'_>, f32, f32)],
    ) -> Result<Vec<Vec<Region>>, String> {
        pages
            .iter()
            .map(|p| Ok(self.run_batch(&[*p])?.pop().expect("one result")))
            .collect()
    }

    fn run_batch(
        &mut self,
        pages: &[(LayoutSrc<'_>, f32, f32)],
    ) -> Result<Vec<Vec<Region>>, String> {
        Self::run_on(&mut self.session, pages)
    }

    fn run_on(
        session: &mut Session,
        pages: &[(LayoutSrc<'_>, f32, f32)],
    ) -> Result<Vec<Vec<Region>>, String> {
        if pages.is_empty() {
            return Ok(Vec::new());
        }
        // Resize each page to 640×640 (RT-DETR ignores aspect ratio), rescale to
        // [0,1], lay out as NCHW. The kernel depends on the source (see
        // [`LayoutSrc`]): the docling-exact page image goes through Pillow's
        // BILINEAR (the RT-DETR processor's kernel, byte-for-byte), raw
        // bitmaps keep the legacy Triangle stretch.
        let n = (SIDE * SIDE) as usize;
        let batch = pages.len();
        let mut data = vec![0f32; batch * 3 * n];
        for (p, (src, _, _)) in pages.iter().enumerate() {
            let resized = match src {
                LayoutSrc::PageImage(img) => crate::resample::pil_resize(
                    img,
                    SIDE,
                    SIDE,
                    crate::resample::PilFilter::Bilinear,
                ),
                LayoutSrc::Raw(img) => {
                    image::imageops::resize(*img, SIDE, SIDE, FilterType::Triangle)
                }
            };
            let page_off = p * 3 * n;
            for (i, px) in resized.pixels().enumerate() {
                data[page_off + i] = px[0] as f32 / 255.0;
                data[page_off + n + i] = px[1] as f32 / 255.0;
                data[page_off + 2 * n + i] = px[2] as f32 / 255.0;
            }
        }
        let input = Tensor::from_array(([batch, 3, SIDE as usize, SIDE as usize], data))
            .map_err(|e| format!("layout: input tensor: {e}"))?;
        let outputs = session
            .run(ort::inputs!["pixel_values" => input])
            .map_err(|e| format!("layout: inference: {e}"))?;
        let (lshape, logits) = outputs["logits"]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("layout: extract logits: {e}"))?;
        let (_, boxes) = outputs["pred_boxes"]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("layout: extract boxes: {e}"))?;

        let num_queries = lshape[1] as usize;
        let num_classes = lshape[2] as usize;

        let mut all = Vec::with_capacity(batch);
        for (p, (_, page_w, page_h)) in pages.iter().enumerate() {
            let logits =
                &logits[p * num_queries * num_classes..(p + 1) * num_queries * num_classes];
            let boxes = &boxes[p * num_queries * 4..(p + 1) * num_queries * 4];
            all.push(decode_layout(
                logits,
                boxes,
                num_queries,
                num_classes,
                *page_w,
                *page_h,
            ));
        }
        Ok(all)
    }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Pack one page image into the model's `(1, 3, SIDE, SIDE)` input: resize
/// (aspect ignored, RT-DETR convention), rescale to `[0,1]`, CHW. Shared
/// with the browser build (#157), which delegates only the session call.
#[cfg(feature = "ocr-prep")]
pub fn layout_input(img: &image::RgbImage) -> Vec<f32> {
    let n = (SIDE * SIDE) as usize;
    let mut data = vec![0f32; 3 * n];
    let resized = image::imageops::resize(img, SIDE, SIDE, image::imageops::FilterType::Triangle);
    for (i, px) in resized.pixels().enumerate() {
        data[i] = px[0] as f32 / 255.0;
        data[n + i] = px[1] as f32 / 255.0;
        data[2 * n + i] = px[2] as f32 / 255.0;
    }
    data
}

/// Decode one page's raw RT-DETR outputs into scored [`Region`]s in page
/// points — sigmoid over every (query, class), top-`num_queries` kept, boxes
/// converted center→corners and scaled. Shared with the browser build; the
/// native batch path calls it per page, so both decode identically.
pub fn decode_layout(
    logits: &[f32],
    boxes: &[f32],
    num_queries: usize,
    num_classes: usize,
    page_w: f32,
    page_h: f32,
) -> Vec<Region> {
    let mut scored: Vec<(f32, usize)> = (0..num_queries * num_classes)
        .map(|idx| (sigmoid(logits[idx]), idx))
        .collect();
    scored.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
    scored.truncate(num_queries);

    let mut regions = Vec::new();
    for (score, idx) in scored {
        if score <= THRESHOLD {
            continue;
        }
        let label_id = idx % num_classes;
        let q = idx / num_classes;
        let cx = boxes[q * 4];
        let cy = boxes[q * 4 + 1];
        let w = boxes[q * 4 + 2];
        let h = boxes[q * 4 + 3];
        // center_to_corners, then scale normalized coords to page points.
        let l = (cx - w / 2.0) * page_w;
        let t = (cy - h / 2.0) * page_h;
        let r = (cx + w / 2.0) * page_w;
        let b = (cy + h / 2.0) * page_h;
        regions.push(Region {
            label: LABELS.get(label_id).copied().unwrap_or("text"),
            score,
            l,
            t,
            r,
            b,
        });
    }
    regions
}
