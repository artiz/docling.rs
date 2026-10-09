//! OCR for a standalone picture (#645): the text the recognizer reads off an
//! image embedded in a non-PDF document — a DOCX/PPTX screenshot, an HTML
//! figure, a sampled video frame — so the picture-OCR enrichment in
//! `docling` can attach it to the picture as docling's description
//! annotation. The models are the ML pipeline's own (the recognizer pair,
//! the text detector, Tesseract under `ocr_engine`), loaded lazily on the
//! first picture exactly as they are for a scanned page, so a converter that
//! OCRs pictures and scans alike holds one set of sessions.
//!
//! The picture is treated as the image input `Pipeline::convert_image` sees:
//! its own scale-1.0 page whose one text region is the whole image, read at
//! docling's effective OCR resolution for images (3 px/pt shrunk to
//! RapidOCR's 2000 px longer side, #570) — no layout pass, since the caller
//! already knows the whole image is the picture. The detector's boxes are
//! the line crops when `ocr_det.onnx` is installed (#570); the ink-projection
//! strips otherwise. Lines under `DOCLING_RS_OCR_TEXT_SCORE` are dropped by
//! the recognizer, as on a page.

use image::RgbImage;

use crate::layout::Region;
use crate::pdfium_backend::TextCell;
use crate::{ocr, ocr_input, page_ocr_scale, EnrichSlot, PdfError, Pipeline};

/// What the OCR read off one picture: the lines in reading order (top to
/// bottom, left to right within a row, rows joined by `\n`) and the engine
/// that read them — docling's `PictureDescriptionData.provenance`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictureText {
    pub text: String,
    /// `ppocr` or `tesseract` — the [`ocr::OcrEngine`] that ran.
    pub provenance: &'static str,
}

impl Pipeline {
    /// Decode an embedded picture's bytes (PNG/JPEG/GIF/BMP/TIFF/WebP) under
    /// the standalone-image pixel cap (`DOCLING_RS_MAX_IMAGE_PIXELS`, 30000
    /// per side) — a crafted header never allocates a multi-GB bitmap.
    pub fn decode_picture(&self, bytes: &[u8]) -> Result<RgbImage, PdfError> {
        crate::decode_image_limited(bytes)
    }

    /// The DocumentFigureClassifier's predictions for a picture (descending
    /// confidence), loading the model on first use. `None` when the model is
    /// not installed (warned once by the loader) or inference failed — the
    /// caller decides what an unclassifiable picture means (the picture-OCR
    /// class filter lets it through).
    pub fn classify_picture(&mut self, img: &RgbImage) -> Option<Vec<docling_core::PictureClass>> {
        let mut guard = self.classifier.lock().unwrap_or_else(|p| p.into_inner());
        if matches!(*guard, EnrichSlot::Unloaded) {
            *guard = match crate::enrich::PictureClassifier::load_with(crate::intra_threads()) {
                Some(m) => EnrichSlot::Ready(m),
                None => EnrichSlot::Missing,
            };
        }
        let EnrichSlot::Ready(model) = &mut *guard else {
            return None;
        };
        match model.classify(img) {
            Ok(classes) => Some(classes),
            Err(e) => {
                eprintln!("docling-pdf: picture classifier: {e}");
                None
            }
        }
    }

    /// OCR a picture: `Ok(None)` when the engine read no text (or could not
    /// load — the recognizer's one-time warning says so, and `skip_ocr`
    /// short-circuits without one), `Err` only for a failed inference.
    pub fn ocr_picture(&mut self, img: &RgbImage) -> Result<Option<PictureText>, PdfError> {
        let provenance = match self.ocr_engine {
            ocr::OcrEngine::PpOcr => "ppocr",
            ocr::OcrEngine::Tesseract => "tesseract",
        };
        let (w, h) = (img.width() as f32, img.height() as f32);
        if w < 1.0 || h < 1.0 {
            return Ok(None);
        }
        // The image is its own page at 1 px per point; the OCR reads it at
        // docling's image resolution unless `ocr_scale` says otherwise.
        let ocr_scale = page_ocr_scale(self.ocr_scale, w, h, 1.0);
        let mut cache = None;
        let (view, scale) = ocr_input(&mut cache, img, 1.0, ocr_scale);
        let region = Region {
            label: "text",
            score: 1.0,
            l: 0.0,
            t: 0.0,
            r: w,
            b: h,
        };
        let worker = self.primary()?;
        let detected = match worker.det_model() {
            Some(det) => Some(det.detect(view).map_err(PdfError::Ocr)?),
            None => None,
        };
        let Some(model) = worker.ocr_model()? else {
            return Ok(None);
        };
        let cells = model
            .ocr_page_with(
                view,
                std::slice::from_ref(&region),
                scale,
                detected.as_deref(),
            )
            .map_err(PdfError::Ocr)?;
        let text = lines_text(cells.into_iter().map(|(c, _)| c).collect());
        Ok((!text.is_empty()).then_some(PictureText { text, provenance }))
    }
}

/// Join OCR line cells into text in reading order: rows top to bottom, cells
/// left to right within a row. A cell joins the row whose vertical extent
/// contains its centre — two columns of a screenshot's dialog read across,
/// the way a human scans it; a line that sits lower starts a new row. Empty
/// cells are dropped.
pub(crate) fn lines_text(mut cells: Vec<TextCell>) -> String {
    cells.retain(|c| !c.text.trim().is_empty());
    cells.sort_by(|a, b| a.t.total_cmp(&b.t).then(a.l.total_cmp(&b.l)));
    let mut rows: Vec<(f32, f32, Vec<TextCell>)> = Vec::new();
    for cell in cells {
        let mid = (cell.t + cell.b) / 2.0;
        match rows.last_mut() {
            Some((t, b, row)) if mid >= *t && mid <= *b => {
                *t = t.min(cell.t);
                *b = b.max(cell.b);
                row.push(cell);
            }
            _ => rows.push((cell.t, cell.b, vec![cell])),
        }
    }
    rows.iter_mut()
        .map(|(_, _, row)| {
            row.sort_by(|a, b| a.l.total_cmp(&b.l));
            row.iter()
                .map(|c| c.text.trim())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(text: &str, l: f32, t: f32, r: f32, b: f32) -> TextCell {
        TextCell {
            text: text.into(),
            l,
            t,
            r,
            b,
        }
    }

    /// Rows read across, then down; a cell whose centre sits below the
    /// current row starts a new one, and empty cells vanish.
    #[test]
    fn lines_text_reads_rows_across_then_down() {
        let cells = vec![
            cell("right", 200.0, 10.0, 260.0, 30.0),
            cell("below", 10.0, 40.0, 80.0, 60.0),
            cell("left", 10.0, 12.0, 60.0, 32.0),
            cell("  ", 10.0, 70.0, 20.0, 80.0),
        ];
        assert_eq!(lines_text(cells), "left right\nbelow");
        assert_eq!(lines_text(Vec::new()), "");
    }
}
