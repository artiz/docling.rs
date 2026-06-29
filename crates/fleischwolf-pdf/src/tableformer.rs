//! TableFormer: table-structure recovery via docling-ibm-models, exported to
//! ONNX by `scripts/export_tableformer.py`. The image encoder + tag-transformer
//! encoder run once to a memory tensor; the decoder is then stepped
//! autoregressively to emit an OTSL structure-token sequence (the same model
//! docling runs). See PDF_CONFORMANCE.md.

use image::RgbImage;
use ort::session::Session;
use ort::value::Tensor;

const SIDE: u32 = 448;
// Verbatim from docling's tm_config.json image_normalization (more digits than
// f32 holds; kept exact for provenance).
#[allow(clippy::excessive_precision)]
const MEAN: [f32; 3] = [0.94247851, 0.94254675, 0.94292611];
#[allow(clippy::excessive_precision)]
const STD: [f32; 3] = [0.17910956, 0.17940403, 0.17931663];
const MAX_STEPS: usize = 1024;

/// OTSL structure tokens (TableModel04_rs wordmap indices).
pub const START: i64 = 2;
pub const END: i64 = 3;
pub const ECEL: i64 = 4; // empty cell
pub const FCEL: i64 = 5; // full (content) cell
pub const LCEL: i64 = 6; // left-looking: extends the cell to its left (colspan)
pub const UCEL: i64 = 7; // up-looking: extends the cell above (rowspan)
pub const XCEL: i64 = 8; // cross: spans both ways
pub const NL: i64 = 9; // new row
pub const CHED: i64 = 10; // column header
pub const RHED: i64 = 11; // row header
pub const SROW: i64 = 12; // section row

pub struct TableFormer {
    encoder: Session,
    decoder: Session,
}

impl TableFormer {
    /// Load the exported encoder/decoder ONNX graphs (env overrides, else
    /// `models/tableformer/{encoder,decoder}.onnx`). Returns `None` if either is
    /// absent, so the pipeline falls back to geometric reconstruction.
    pub fn load() -> Option<Self> {
        let enc = std::env::var("DOCLING_TABLEFORMER_ENCODER")
            .unwrap_or_else(|_| "models/tableformer/encoder.onnx".to_string());
        let dec = std::env::var("DOCLING_TABLEFORMER_DECODER")
            .unwrap_or_else(|_| "models/tableformer/decoder.onnx".to_string());
        if !std::path::Path::new(&enc).exists() || !std::path::Path::new(&dec).exists() {
            return None;
        }
        let build = |path: &str| -> Result<Session, String> {
            Session::builder()
                .map_err(|e| e.to_string())?
                .with_intra_threads(crate::intra_threads())
                .map_err(|e| e.to_string())?
                .commit_from_file(path)
                .map_err(|e| format!("tableformer load {path}: {e}"))
        };
        match (build(&enc), build(&dec)) {
            (Ok(encoder), Ok(decoder)) => Some(Self { encoder, decoder }),
            _ => None,
        }
    }

    /// Predict the OTSL structure-token sequence for a table-region image.
    pub fn predict_otsl(&mut self, img: &RgbImage) -> Result<Vec<i64>, String> {
        // Preprocess exactly as docling: bilinear (cv2.INTER_LINEAR) resize the
        // crop to 448², normalize (x/255 − mean)/std, laid out as (C, W, H) —
        // docling transposes (2,1,0), so width is the major spatial axis (not
        // C,H,W). The page→1024px box-average (cv2.INTER_AREA) is the caller's.
        let n = (SIDE * SIDE) as usize;
        let side = SIDE as usize;
        let (sw, sh) = (img.width() as i32, img.height() as i32);
        let sxr = sw as f32 / SIDE as f32;
        let syr = sh as f32 / SIDE as f32;
        let mut data = vec![0f32; 3 * n];
        for h in 0..side {
            let fy = (h as f32 + 0.5) * syr - 0.5;
            let wy = fy - fy.floor();
            let y0c = (fy.floor() as i32).clamp(0, sh - 1) as u32;
            let y1c = (fy.floor() as i32 + 1).clamp(0, sh - 1) as u32;
            for w in 0..side {
                let fx = (w as f32 + 0.5) * sxr - 0.5;
                let wx = fx - fx.floor();
                let x0c = (fx.floor() as i32).clamp(0, sw - 1) as u32;
                let x1c = (fx.floor() as i32 + 1).clamp(0, sw - 1) as u32;
                let p00 = img.get_pixel(x0c, y0c);
                let p01 = img.get_pixel(x1c, y0c);
                let p10 = img.get_pixel(x0c, y1c);
                let p11 = img.get_pixel(x1c, y1c);
                let idx = w * side + h; // (C, W, H): c*n + w*H + h
                for c in 0..3 {
                    let top = p00[c] as f32 * (1.0 - wx) + p01[c] as f32 * wx;
                    let bot = p10[c] as f32 * (1.0 - wx) + p11[c] as f32 * wx;
                    let v = top * (1.0 - wy) + bot * wy;
                    data[c * n + idx] = (v / 255.0 - MEAN[c]) / STD[c];
                }
            }
        }
        let input = Tensor::from_array(([1usize, 3, SIDE as usize, SIDE as usize], data))
            .map_err(|e| format!("tableformer: input: {e}"))?;
        let enc_out = self
            .encoder
            .run(ort::inputs!["image" => input])
            .map_err(|e| format!("tableformer: encode: {e}"))?;
        let (mshape, mem) = enc_out["memory"]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("tableformer: memory: {e}"))?;
        let mshape: Vec<usize> = mshape.iter().map(|&x| x as usize).collect();
        let mem: Vec<f32> = mem.to_vec();

        // Autoregressive decode: the decoder graph re-applies the layers to the
        // whole prefix under a causal mask (statelessly reproducing the model's
        // per-layer cache), so we just feed the growing token list back in. The
        // two structure corrections mirror docling's `predict` exactly — note its
        // `line_num` is never incremented, so `xcel→lcel` applies on every row.
        let mut tags: Vec<i64> = vec![START];
        let mut out: Vec<i64> = Vec::new();
        let mut prev_ucel = false;
        while out.len() < MAX_STEPS {
            let tags_t = Tensor::from_array(([tags.len(), 1usize], tags.clone()))
                .map_err(|e| format!("tableformer: tags: {e}"))?;
            let mem_t = Tensor::from_array((mshape.clone(), mem.clone()))
                .map_err(|e| format!("tableformer: mem: {e}"))?;
            let dout = self
                .decoder
                .run(ort::inputs!["tags" => tags_t, "memory" => mem_t])
                .map_err(|e| format!("tableformer: decode: {e}"))?;
            let (_, logits) = dout["logits"]
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("tableformer: logits: {e}"))?;
            let mut tag = argmax(logits) as i64;
            if tag == XCEL {
                tag = LCEL;
            }
            if prev_ucel && tag == LCEL {
                tag = FCEL;
            }
            if tag == END {
                break;
            }
            out.push(tag);
            tags.push(tag);
            prev_ucel = tag == UCEL;
        }
        Ok(out)
    }
}

fn argmax(v: &[f32]) -> usize {
    v.iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)
        .unwrap_or(0)
}
