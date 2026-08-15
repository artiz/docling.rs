//! OCR for scanned pages, via the PP-OCRv3 recognition model (CRNN/SVTR) run
//! with `ort`. The layout model already locates text regions on the page image
//! (it works without a text layer), so OCR only needs *recognition*: each text
//! region is cropped, split into lines by horizontal projection, and each line
//! is recognised and decoded with CTC — producing [`TextCell`]s the normal
//! layout assembly then consumes. This avoids a separate text-detection model.

use image::RgbImage;
use ort::session::Session;
use ort::value::Tensor;

use crate::layout::Region;
// The ONNX-free half (line prep, batching, CTC decode) lives in `ocr_prep`
// so the wasm build shares it verbatim (#79 phase 2).
use crate::ocr_prep::{
    batch_input, decode_row_scored, dict_chars, prep_region_lines, prep_table_words, width_batches,
    PrepLine, REC_HEIGHT,
};
use crate::pdfium_backend::TextCell;

pub struct OcrModel {
    rec: Session,
    /// CTC classes: index 0 = blank, 1..=6623 = dictionary, 6624 = space.
    chars: Vec<String>,
}

/// OCR recognition language: which PP-OCRv3 model + dictionary pair runs.
///
/// The default is **English** (`.models/ocr_rec_en.onnx` + `.models/en_dict.txt`):
/// the multilingual `ch_` model reads Latin scripts with badly degraded word
/// spacing (glued words on ordinary English scans), which is the common
/// real-world case. `Ch` selects the `ch_` pair (`.models/ocr_rec.onnx` +
/// `.models/ppocr_keys_v1.txt`) — that is what upstream docling conformance is
/// measured with, and `scripts/conformance/pdf_*.sh` pin it explicitly (by
/// path, which wins over this selector).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OcrLang {
    /// en_PP-OCRv3 — English-only, proper Latin word spacing.
    #[default]
    En,
    /// ch_PP-OCRv3 — multilingual; the docling-conformance model.
    Ch,
}

impl OcrLang {
    /// Parse a user-supplied language id. `None` for anything but `en`/`ch`
    /// (trimmed, case-insensitive) — callers surface their own error/warning.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "en" => Some(Self::En),
            "ch" => Some(Self::Ch),
            _ => None,
        }
    }

    /// The process-level choice from `DOCLING_RS_OCR_LANG` (empty/unset → the
    /// English default; unknown values warn and use English).
    pub fn from_env() -> Self {
        let Some(raw) = docling_core::env::nonempty("DOCLING_RS_OCR_LANG") else {
            return Self::default();
        };
        Self::parse(&raw).unwrap_or_else(|| {
            eprintln!("docling-pdf: DOCLING_RS_OCR_LANG={raw:?} is not en|ch; using en");
            Self::default()
        })
    }
}

/// Which document regions feed the OCR — docling 2.116's `OcrMode` (#254,
/// upstream docling#3710). Upstream restructured its pipeline so OCR runs
/// *after* layout, on layout regions filtered by the PDF text layer — the
/// architecture this port has always had — and named the strategies:
///
/// - `PdfAwareLayoutRegions` (upstream's **default**): OCR only layout regions
///   the embedded text layer can't cover. Exactly the standard path here —
///   scanned pages OCR their regions, digital pages OCR only text-less bitmap
///   areas.
/// - `FullPage` / `LayoutRegions`: ignore the PDF text layer and OCR
///   everything. Both map onto the [`force_full_page_ocr`] machinery (discard
///   the text layer, OCR every layout region): the upstream distinction —
///   whole-page vs per-region *detector* input — has no analogue in this
///   engine, whose PP-OCR recognizer always consumes per-region line crops.
///
/// [`force_full_page_ocr`]: crate::Pipeline::force_full_page_ocr
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OcrMode {
    /// Upstream's `default`: currently wired to `PdfAwareLayoutRegions`.
    #[default]
    Default,
    /// OCR the full page, text layer ignored (docling's `full_page`; the
    /// mode-shaped spelling of `force_full_page_ocr`).
    FullPage,
    /// OCR every layout region, text layer ignored (docling's
    /// `layout_regions`).
    LayoutRegions,
    /// OCR layout regions the text layer can't cover (docling's
    /// `pdf_aware_layout_regions` — the default behavior).
    PdfAwareLayoutRegions,
}

impl OcrMode {
    /// Parse docling's mode ids. `None` for anything else — callers surface
    /// their own error/warning.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "default" => Some(Self::Default),
            "full_page" => Some(Self::FullPage),
            "layout_regions" => Some(Self::LayoutRegions),
            "pdf_aware_layout_regions" => Some(Self::PdfAwareLayoutRegions),
            _ => None,
        }
    }

    /// The process-level choice from `DOCLING_RS_OCR_MODE` (empty/unset → the
    /// default; unknown values warn and use the default).
    pub fn from_env() -> Self {
        let Some(raw) = docling_core::env::nonempty("DOCLING_RS_OCR_MODE") else {
            return Self::default();
        };
        Self::parse(&raw).unwrap_or_else(|| {
            eprintln!(
                "docling-pdf: DOCLING_RS_OCR_MODE={raw:?} is not \
                 default|full_page|layout_regions|pdf_aware_layout_regions; using default"
            );
            Self::default()
        })
    }

    /// Whether this mode discards the embedded text layer — the engine truth
    /// both non-default modes reduce to.
    pub fn forces_full_page(self) -> bool {
        matches!(self, Self::FullPage | Self::LayoutRegions)
    }
}

/// The process-level OCR render scale from `DOCLING_RS_OCR_SCALE` (#254,
/// upstream docling#3877's `OcrOptions.scale`): pixels per PDF point fed to
/// the recognizer. Unset/empty → `None` (OCR reads the pipeline's own page
/// render, 2.0 px/pt); non-positive or unparsable values warn and are ignored.
pub fn scale_from_env() -> Option<f32> {
    let raw = docling_core::env::nonempty("DOCLING_RS_OCR_SCALE")?;
    match raw.parse::<f32>() {
        Ok(s) if s > 0.0 && s.is_finite() => Some(s),
        _ => {
            eprintln!(
                "docling-pdf: DOCLING_RS_OCR_SCALE={raw:?} is not a positive number; ignored"
            );
            None
        }
    }
}

/// Resolve the recognition model + dictionary pair for `lang`. An English
/// default that isn't on disk (older model checkouts) degrades to the `ch_`
/// pair with a warning rather than failing — the usual missing-optional-asset
/// convention. Explicit `DOCLING_OCR_REC_ONNX` / `DOCLING_OCR_DICT` paths win
/// over all of this; they are a pair, so set both together.
pub(crate) fn resolve_rec_pair(lang: OcrLang) -> (String, String) {
    const CH: (&str, &str) = (".models/ocr_rec.onnx", ".models/ppocr_keys_v1.txt");
    const EN: (&str, &str) = (".models/ocr_rec_en.onnx", ".models/en_dict.txt");
    let want_ch = lang == OcrLang::Ch;
    let pick = if want_ch { CH } else { EN };
    let (mut rec, mut dict) = (crate::resolve_asset(pick.0), crate::resolve_asset(pick.1));
    if !want_ch && (!std::path::Path::new(&rec).exists() || !std::path::Path::new(&dict).exists()) {
        let (ch_rec, ch_dict) = (crate::resolve_asset(CH.0), crate::resolve_asset(CH.1));
        if std::path::Path::new(&ch_rec).exists() && std::path::Path::new(&ch_dict).exists() {
            eprintln!(
                "docling-pdf: English OCR model not found ({rec}); falling back to the \
                 multilingual ch_ model — expect weak Latin word spacing. Fetch it with \
                 scripts/install/download_dependencies.sh"
            );
            (rec, dict) = (ch_rec, ch_dict);
        }
    }
    (
        docling_core::env::nonempty("DOCLING_OCR_REC_ONNX").unwrap_or(rec),
        docling_core::env::nonempty("DOCLING_OCR_DICT").unwrap_or(dict),
    )
}

impl OcrModel {
    /// Load the recognition model and its character dictionary for `lang` —
    /// see [`resolve_rec_pair`] for the selection rules (explicit
    /// `DOCLING_OCR_REC_ONNX`/`DOCLING_OCR_DICT` paths win).
    pub fn load(lang: OcrLang) -> Result<Self, String> {
        let (rec_path, dict_path) = resolve_rec_pair(lang);
        // Single-threaded: ORT's multi-threaded float-reduction order varies
        // across runs, which flips the CTC argmax on low-confidence characters
        // (e.g. noisy faxes) and makes the snapshot output non-deterministic. The
        // recognition inputs are tiny per-line crops, so the throughput cost is
        // negligible.
        let builder = Session::builder()
            .map_err(|e| format!("ocr: builder: {e}"))?
            .with_intra_threads(1)
            .map_err(|e| format!("ocr: intra_threads: {e}"))?;
        let rec = crate::ep::apply(builder)
            .map_err(|e| format!("ocr: {e}"))?
            .commit_from_file(&rec_path)
            .map_err(|e| format!("ocr: load {rec_path}: {e}"))?;
        let dict = std::fs::read_to_string(&dict_path)
            .map_err(|e| format!("ocr: read dict {dict_path}: {e}"))?;
        Ok(Self {
            rec,
            chars: dict_chars(&dict),
        })
    }

    /// Recognise a batch of prepared *same-width* lines in one session run.
    ///
    /// Only equal widths ever share a run: same-width batching is
    /// bit-identical to one-at-a-time recognition (each sample keeps its own
    /// data and per-sample kernel reduction order — verified empirically on
    /// the scanned corpus), whereas width-padding leaks into the real
    /// timesteps through the model's global-attention blocks and measurably
    /// changes low-confidence characters.
    fn recognize_batch(
        &mut self,
        w: usize,
        chunk: &[usize],
        lines: &[PrepLine],
    ) -> Result<Vec<(String, f32)>, String> {
        let n = chunk.len();
        let data = batch_input(w, chunk, lines);
        let input = Tensor::from_array(([n, 3, REC_HEIGHT as usize, w], data))
            .map_err(|e| format!("ocr: input tensor: {e}"))?;
        let outputs = self
            .rec
            .run(ort::inputs!["x" => input])
            .map_err(|e| format!("ocr: rec inference: {e}"))?;
        let (shape, probs) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("ocr: extract rec: {e}"))?;
        let t_len = shape[1] as usize;
        let nc = shape[2] as usize;
        Ok((0..n)
            .map(|i| {
                decode_row_scored(
                    &self.chars,
                    &probs[i * t_len * nc..(i + 1) * t_len * nc],
                    nc,
                )
            })
            .collect())
    }

    /// Recognize `lines` and reduce to orientation-probe evidence: the
    /// confidence-weighted character count `Σ(conf × chars)` plus the raw
    /// character total (#225). Same deterministic width-batching as page OCR.
    pub(crate) fn score_lines(&mut self, lines: &[PrepLine]) -> Result<(f32, usize), String> {
        let mut weighted = 0.0f32;
        let mut chars = 0usize;
        for (w, chunk) in width_batches(lines) {
            for (text, conf) in self.recognize_batch(w, &chunk, lines)? {
                let n = text.trim().chars().count();
                weighted += conf * n as f32;
                chars += n;
            }
        }
        Ok((weighted, chars))
    }

    /// OCR a page: produce text cells (page points) for every line found inside
    /// the text regions, each paired with its recognition confidence (mean
    /// emitted-character probability — feeds the page `ocr_score`, #183).
    /// `scale` is image-px per page-point.
    pub fn ocr_page(
        &mut self,
        img: &RgbImage,
        regions: &[Region],
        scale: f32,
    ) -> Result<Vec<(TextCell, f32)>, String> {
        // Gather every line crop on the page first (shared with the browser
        // path), so equal-width lines can share a recognition run regardless
        // of which region they came from.
        let (bboxes, lines) = prep_region_lines(img, regions, scale);

        // Deterministic width-batching (shared with the wasm path).
        let mut texts = vec![(String::new(), 0.0f32); lines.len()];
        for (w, chunk) in width_batches(&lines) {
            for (&i, text) in chunk.iter().zip(self.recognize_batch(w, &chunk, &lines)?) {
                texts[i] = text;
            }
        }

        // Emit cells in page order, exactly as the sequential walk did.
        let mut cells = Vec::new();
        for ((l, t, r, b), (text, conf)) in bboxes.into_iter().zip(texts) {
            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            cells.push((TextCell { text, l, t, r, b }, conf));
        }
        Ok(cells)
    }

    /// Recognize the *word* crops inside the page's table regions (mirroring
    /// the browser scanned path): [`ocr_page`](Self::ocr_page) deliberately
    /// skips table labels, so a scanned table would otherwise reach the cell
    /// matcher with no words at all and dissolve (#173). Returns word-level
    /// [`TextCell`]s in page points.
    pub fn ocr_table_words(
        &mut self,
        img: &RgbImage,
        regions: &[Region],
        scale: f32,
    ) -> Result<Vec<(TextCell, f32)>, String> {
        let (bboxes, lines) = prep_table_words(img, regions, scale);
        let mut texts = vec![(String::new(), 0.0f32); lines.len()];
        for (w, chunk) in width_batches(&lines) {
            for (&i, text) in chunk.iter().zip(self.recognize_batch(w, &chunk, &lines)?) {
                texts[i] = text;
            }
        }
        let mut cells = Vec::new();
        for ((l, t, r, b), (text, conf)) in bboxes.into_iter().zip(texts) {
            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            cells.push((TextCell { text, l, t, r, b }, conf));
        }
        Ok(cells)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #254: docling's four `OcrMode` ids parse; `full_page`/`layout_regions`
    /// reduce to the force-full-page machinery, the default/pdf-aware pair to
    /// the standard text-layer-aware path. Unknown ids parse to nothing.
    #[test]
    fn ocr_mode_ids_parse_and_map_to_forcing() {
        for (id, mode, forces) in [
            ("default", OcrMode::Default, false),
            ("full_page", OcrMode::FullPage, true),
            ("layout_regions", OcrMode::LayoutRegions, true),
            (
                "pdf_aware_layout_regions",
                OcrMode::PdfAwareLayoutRegions,
                false,
            ),
        ] {
            assert_eq!(OcrMode::parse(id), Some(mode));
            assert_eq!(mode.forces_full_page(), forces, "{id}");
        }
        assert_eq!(OcrMode::parse(" Full_Page "), Some(OcrMode::FullPage));
        assert_eq!(OcrMode::parse("easyocr"), None);
        assert_eq!(OcrMode::parse(""), None);
    }
}
