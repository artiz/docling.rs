//! Text-line orientation classifier (#666): RapidOCR's `TextClassifier`
//! with PaddleOCR's `PP-LCNet_x0_25_textline_ori` (`Cls.ocr_version:
//! PP-OCRv5` — RapidOCR's `ch_PP-LCNet_x0_25_textline_ori_cls_mobile.onnx`,
//! Apache-2.0), run on the oriented line crops when
//! `ocr_textline_orientation` is on: a line the model reads as upside down
//! (`180` above [`CLS_THRESH`]) is turned 180° before recognition.
//!
//! Pre/post-processing follow `rapidocr/ch_ppocr_cls`: each crop resized
//! (bilinear, aspect kept) to height [`CLS_H`], width capped at [`CLS_W`],
//! `(x/255 − 0.5)/0.5` in BGR order (cv2 input), zero-padded on the right;
//! crops classified in batches of [`CLS_BATCH`] sorted by aspect ratio; the
//! label is the argmax of `["0", "180"]`. Model: `.models/ocr_textline_ori.onnx`
//! (`DOCLING_OCR_TEXTLINE_ORI_ONNX` overrides).

use image::RgbImage;

/// The classifier's input height (`CLS_SHAPE_BY_OCR_VERSION[PPOCRV5]`).
pub const CLS_H: u32 = 80;
/// The classifier's input width.
pub const CLS_W: u32 = 160;
/// `cls_batch_num`.
pub const CLS_BATCH: usize = 6;
/// `cls_thresh`: the `180` score a line must beat to be turned.
pub const CLS_THRESH: f32 = 0.9;

/// `TextClassifier.resize_norm_img`: one crop as the `3 × CLS_H × CLS_W`
/// CHW input, BGR, normalized to `[-1, 1]`, zero right of the resized width.
pub fn prep_cls(crop: &RgbImage) -> Vec<f32> {
    let (w, h) = crop.dimensions();
    let n = (CLS_H * CLS_W) as usize;
    let mut data = vec![0f32; 3 * n];
    if w == 0 || h == 0 {
        return data;
    }
    let ratio = w as f32 / h as f32;
    let rw = ((CLS_H as f32 * ratio).ceil() as u32).clamp(1, CLS_W);
    let resized = crate::ocr_det::resize_bilinear(crop, rw, CLS_H);
    for (x, y, px) in resized.enumerate_pixels() {
        let i = (y * CLS_W + x) as usize;
        // BGR: channel 0 is blue.
        data[i] = px[2] as f32 / 127.5 - 1.0;
        data[n + i] = px[1] as f32 / 127.5 - 1.0;
        data[2 * n + i] = px[0] as f32 / 127.5 - 1.0;
    }
    data
}

/// RapidOCR's batch order: crop indices sorted by aspect ratio (w / h),
/// then cut into [`CLS_BATCH`]-sized batches.
pub fn cls_batches(crops: &[&RgbImage]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..crops.len()).collect();
    let ratio = |i: usize| crops[i].width() as f32 / crops[i].height().max(1) as f32;
    order.sort_by(|&a, &b| ratio(a).total_cmp(&ratio(b)));
    order.chunks(CLS_BATCH).map(<[usize]>::to_vec).collect()
}

/// `ClsPostProcess` + the turn rule: whether a `[p0, p180]` row says the
/// line is upside down.
pub fn is_upside_down(probs: &[f32]) -> bool {
    matches!(probs, [p0, p180] if p180 > p0 && *p180 > CLS_THRESH)
}

/// `DOCLING_OCR_TEXTLINE_ORI_ONNX`, else `.models/ocr_textline_ori.onnx`
/// through the asset resolver (CWD, `DOCLING_RS_MODELS_DIR`, exe dir).
#[cfg(feature = "ml")]
pub(crate) fn resolve_cls_path() -> String {
    docling_core::env::nonempty("DOCLING_OCR_TEXTLINE_ORI_ONNX")
        .unwrap_or_else(|| crate::resolve_asset(".models/ocr_textline_ori.onnx"))
}

#[cfg(feature = "ml")]
pub use session::TextlineCls;

#[cfg(feature = "ml")]
mod session {
    use super::{cls_batches, is_upside_down, prep_cls, resolve_cls_path, CLS_H, CLS_W};
    use image::RgbImage;
    use ort::session::Session;
    use ort::value::Tensor;

    /// The classifier session: one intra-op thread, so its scores — and
    /// with them which lines turn — do not vary across runs.
    pub struct TextlineCls {
        session: Session,
    }

    impl TextlineCls {
        pub fn load() -> Result<Self, String> {
            let path = resolve_cls_path();
            if !std::path::Path::new(&path).exists() {
                return Err(format!("text-line orientation model not found at {path}"));
            }
            let builder = docling_onnx::session_builder()
                .map_err(|e| format!("ocr-cls: builder: {e}"))?
                .with_intra_threads(1)
                .map_err(|e| format!("ocr-cls: intra_threads: {e}"))?;
            let builder = docling_onnx::apply(builder).map_err(|e| format!("ocr-cls: {e}"))?;
            let session = docling_onnx::commit(builder, &path, "textline_ori")
                .map_err(|e| format!("ocr-cls: load {path}: {e}"))?;
            Ok(Self { session })
        }

        /// Which of `crops` are upside down, by index.
        pub fn upside_down(&mut self, crops: &[&RgbImage]) -> Result<Vec<bool>, String> {
            let mut out = vec![false; crops.len()];
            let name = self.session.inputs()[0].name().to_string();
            for batch in cls_batches(crops) {
                let data: Vec<f32> = batch.iter().flat_map(|&i| prep_cls(crops[i])).collect();
                let input =
                    Tensor::from_array(([batch.len(), 3, CLS_H as usize, CLS_W as usize], data))
                        .map_err(|e| format!("ocr-cls: input: {e}"))?;
                let outputs =
                    docling_onnx::run(&mut self.session, ort::inputs![name.as_str() => input])
                        .map_err(|e| format!("ocr-cls: run: {e}"))?;
                let (shape, probs) = outputs[0]
                    .try_extract_tensor::<f32>()
                    .map_err(|e| format!("ocr-cls: output: {e}"))?;
                let nc = shape.get(1).copied().unwrap_or(0) as usize;
                if nc != 2 {
                    return Err(format!("ocr-cls: unexpected output shape {shape:?}"));
                }
                for (k, &i) in batch.iter().enumerate() {
                    out[i] = is_upside_down(&probs[k * nc..(k + 1) * nc]);
                }
            }
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    /// A 40 × 10 crop resizes to 80 × 320 → capped at 160 wide; a 10 × 10
    /// one to 80 × 80, zero-padded to 160. BGR: a pure-red pixel lands in
    /// the third plane.
    #[test]
    fn prep_resizes_to_80_high_caps_at_160_and_pads_with_zero() {
        let red = RgbImage::from_pixel(10, 10, Rgb([255, 0, 0]));
        let data = prep_cls(&red);
        let n = (CLS_H * CLS_W) as usize;
        assert_eq!(data.len(), 3 * n);
        // Inside the resized 80 px: B = −1, G = −1, R = +1.
        assert_eq!((data[0], data[n], data[2 * n]), (-1.0, -1.0, 1.0));
        // Right of it: the zero padding.
        assert_eq!(
            (data[100], data[n + 100], data[2 * n + 100]),
            (0.0, 0.0, 0.0)
        );
        let wide = RgbImage::from_pixel(40, 10, Rgb([255, 255, 255]));
        let data = prep_cls(&wide);
        assert!(
            data[..n].iter().all(|&v| v == 1.0),
            "a capped crop fills the width"
        );
    }

    #[test]
    fn batches_follow_the_aspect_ratio() {
        let crops: Vec<RgbImage> = [(30, 10), (10, 10), (20, 10)]
            .iter()
            .map(|&(w, h)| RgbImage::new(w, h))
            .collect();
        let refs: Vec<&RgbImage> = crops.iter().collect();
        assert_eq!(cls_batches(&refs), vec![vec![1, 2, 0]]);
        let many: Vec<&RgbImage> = std::iter::repeat_n(&crops[0], 7).collect();
        assert_eq!(
            cls_batches(&many).iter().map(Vec::len).collect::<Vec<_>>(),
            [6, 1]
        );
    }

    #[test]
    fn only_a_confident_180_turns_a_line() {
        assert!(is_upside_down(&[0.05, 0.95]));
        assert!(!is_upside_down(&[0.15, 0.85]));
        assert!(!is_upside_down(&[0.95, 0.05]));
    }
}
