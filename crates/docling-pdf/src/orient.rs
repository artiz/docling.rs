//! Content-based page-orientation detection for the OCR path (#225).
//!
//! `/Rotate` normalization (see `pdfium_backend::extract_page`) only helps
//! when the rotation is *declared*: a phone photo taken sideways or a sheet
//! fed into the scanner in landscape has `/Rotate 0` and looks upright to the
//! PDF layer, so layout and OCR run on a sideways/upside-down raster and
//! recognize noise — silently. This module detects the raster's own
//! orientation and reports the angle to un-rotate by, composing with the
//! metadata normalization through the same [`PdfPage::unrotate`] machinery.
//!
//! There is no orientation model to run (the pipeline deliberately has no
//! text-*detection* network — layout is the detector, and it is exactly what
//! fails on a sideways page). Instead the recognizer itself is the judge, the
//! classic OSD trick: segment the page into line crops (the pure-Rust
//! projection segmentation OCR already uses), recognize a handful of the
//! widest lines under each orientation hypothesis, and score each hypothesis
//! by how much confident text falls out. Upright text decodes many characters
//! at high confidence; sideways/upside-down text decodes few, badly. An
//! upright page early-exits after scoring only its own hypothesis, so the
//! common case pays a few small recognition runs; the probe count is capped,
//! so cost does not grow with page density.
//!
//! Scores are deterministic (single-threaded recognition, fixed probe
//! selection), so pinned snapshots stay stable. Failures degrade to "assume
//! upright" — detection must never make a conversion worse than not having
//! run at all. `DOCLING_RS_OCR_ORIENTATION=off` disables the pass;
//! `DOCLING_RS_DEBUG=1` prints per-hypothesis scores.
//!
//! [`PdfPage::unrotate`]: crate::PdfPage::unrotate

use image::imageops::{rotate180, rotate270, rotate90};
use image::RgbImage;

use crate::layout::Region;
use crate::ocr::OcrModel;
use crate::ocr_prep::{prep_region_lines, PrepLine};
use docling_core::debug_log;

/// Whether the detection pass runs (`DOCLING_RS_OCR_ORIENTATION`, default
/// `auto`; `off`/`0`/`false` disable, anything else warns and stays auto).
pub(crate) fn enabled() -> bool {
    let raw = docling_core::env::nonempty("DOCLING_RS_OCR_ORIENTATION").unwrap_or_default();
    let v = raw.trim().to_ascii_lowercase();
    match v.as_str() {
        "" | "auto" | "on" | "1" | "true" => true,
        "off" | "0" | "false" | "none" => false,
        _ => {
            eprintln!(
                "docling-pdf: DOCLING_RS_OCR_ORIENTATION={raw:?} is not auto|off; using auto"
            );
            true
        }
    }
}

/// How many line crops one hypothesis recognizes at most. The widest lines
/// carry the most characters — a handful is plenty of signal, and the cap
/// keeps the pass O(1) recognition runs regardless of page size.
const PROBES: usize = 6;

/// Accept "upright" without probing the other three hypotheses when the 0°
/// probe alone reads at least this confidently. Covers the overwhelmingly
/// common case (a correctly scanned page) at the cost of one probe round.
const UPRIGHT_CONF: f32 = 0.90;
const UPRIGHT_CHARS: usize = 20;

/// A rotated hypothesis must beat 0° by this factor to overturn it: the
/// recognizer is noisy on garbage input, and a no-op must stay the default
/// when the signal is thin.
const OVERTURN: f32 = 1.2;
/// ...and clear this floor: a page whose *best* hypothesis still reads almost
/// nothing (blank page, pure line-art) has no orientation evidence at all.
const MIN_CHARS: usize = 8;
const MIN_CONF: f32 = 0.55;

/// One hypothesis' evidence: Σ(confidence × chars) over its probe lines, and
/// the raw character count.
struct Score {
    weighted: f32,
    chars: usize,
}

impl Score {
    fn mean_conf(&self) -> f32 {
        if self.chars == 0 {
            0.0
        } else {
            self.weighted / self.chars as f32
        }
    }
}

/// Segment `img` into line crops and score the `PROBES` widest through the
/// recognizer.
fn probe(img: &RgbImage, ocr: &mut OcrModel) -> Result<Score, String> {
    // The whole page as one text region, in image-pixel "points" (scale 1.0):
    // the same projection segmentation OCR uses per layout region, minus the
    // layout model — which cannot be trusted on the very pages this pass is
    // for.
    let page = Region {
        label: "text",
        score: 1.0,
        l: 0.0,
        t: 0.0,
        r: img.width() as f32,
        b: img.height() as f32,
    };
    let (_, mut lines) = prep_region_lines(img, std::slice::from_ref(&page), 1.0);
    // Widest first — most characters per recognition run. Stable order (width,
    // then original index) keeps the selection deterministic.
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by(|&a, &b| lines[b].w.cmp(&lines[a].w).then(a.cmp(&b)));
    order.truncate(PROBES);
    order.sort_unstable();
    // Extract by descending index so earlier indices stay valid.
    let probes: Vec<PrepLine> = order.iter().rev().map(|&i| lines.swap_remove(i)).collect();
    let (weighted, chars) = ocr.score_lines(&probes)?;
    Ok(Score { weighted, chars })
}

/// Detect the clockwise angle the page content is rotated by in the raster
/// (`0`/`90`/`180`/`270`); un-rotating by the returned angle makes it upright.
/// Any internal failure returns 0 — the page converts as-is, exactly as it
/// would have without this pass.
pub(crate) fn detect(img: &RgbImage, ocr: &mut OcrModel) -> u16 {
    let fail = |e: String| {
        debug_log!("docling-pdf: orientation probe failed ({e}); assuming upright");
        0
    };
    let s0 = match probe(img, ocr) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    if s0.chars >= UPRIGHT_CHARS && s0.mean_conf() >= UPRIGHT_CONF {
        debug_log!(
            "docling-pdf: orientation 0° reads {} chars at {:.2} — upright, no probes",
            s0.chars,
            s0.mean_conf()
        );
        return 0;
    }
    // Hypothesis "content is rotated `deg`° clockwise" ⇒ test the bitmap
    // un-rotated by `deg` (the inverse), same convention as `PdfPage::unrotate`.
    let hypotheses: [(u16, RgbImage); 3] = [
        (90, rotate270(img)),
        (180, rotate180(img)),
        (270, rotate90(img)),
    ];
    debug_log!(
        "docling-pdf: orientation 0°: {} chars at {:.2} (weighted {:.1})",
        s0.chars,
        s0.mean_conf(),
        s0.weighted
    );
    let (mut best_deg, mut best) = (
        0u16,
        Score {
            weighted: 0.0,
            chars: 0,
        },
    );
    for (deg, rotated) in &hypotheses {
        let s = match probe(rotated, ocr) {
            Ok(s) => s,
            Err(e) => return fail(e),
        };
        debug_log!(
            "docling-pdf: orientation {deg}°: {} chars at {:.2} (weighted {:.1})",
            s.chars,
            s.mean_conf(),
            s.weighted
        );
        if s.weighted > best.weighted {
            (best_deg, best) = (*deg, s);
        }
    }
    // Overturn "upright" only on clear evidence: the winner must read real
    // text (floors) *and* beat the 0° hypothesis by a margin.
    if best_deg != 0
        && best.chars >= MIN_CHARS
        && best.mean_conf() >= MIN_CONF
        && best.weighted > OVERTURN * s0.weighted
    {
        return best_deg;
    }
    0
}
