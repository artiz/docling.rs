//! Pixel-exact reimplementations of the OpenCV resize kernels docling uses for
//! TableFormer preprocessing, so the model sees byte-identical input. Verified
//! against cv2 on docling's own bitmaps (INTER_AREA max diff 1/255, INTER_LINEAR
//! < 1e-4 in float).

use image::RgbImage;

/// Per-output-pixel source spans + overlap weights for area resampling.
fn area_weights(src: usize, dst: usize, scale: f64) -> Vec<Vec<(usize, f64)>> {
    (0..dst)
        .map(|d| {
            let f1 = d as f64 * scale;
            let f2 = (d + 1) as f64 * scale;
            let s1 = f1.floor() as usize;
            let s2 = (f2.ceil() as usize).min(src);
            (s1..s2)
                .map(|si| {
                    let w = (((si + 1) as f64).min(f2) - (si as f64).max(f1)) / scale;
                    (si, w)
                })
                .collect()
        })
        .collect()
}

/// `cv2.resize(..., interpolation=INTER_AREA)` for shrinking — area-weighted
/// averaging, separable (horizontal then vertical), f64 accumulation.
pub fn inter_area(src: &RgbImage, dw: u32, dh: u32) -> RgbImage {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let (dwu, dhu) = (dw as usize, dh as usize);
    let hw = area_weights(sw, dwu, sw as f64 / dw as f64);
    let vw = area_weights(sh, dhu, sh as f64 / dh as f64);

    // Cache-friendly passes over the raw RGB buffer. The per-pixel addition
    // order is exactly the loop-nest transpose of the naive form, so the f64
    // accumulation — and thus the rounded output — stays bit-identical (the
    // pixel-exactness contract above).
    let raw = src.as_raw();
    let mut tmp = vec![[0f64; 3]; sh * dwu]; // (sh × dw)
    for y in 0..sh {
        let src_row = &raw[y * sw * 3..(y + 1) * sw * 3];
        let dst_row = &mut tmp[y * dwu..(y + 1) * dwu];
        for (acc, ws) in dst_row.iter_mut().zip(hw.iter()) {
            for &(si, w) in ws {
                let p = &src_row[si * 3..si * 3 + 3];
                acc[0] += p[0] as f64 * w;
                acc[1] += p[1] as f64 * w;
                acc[2] += p[2] as f64 * w;
            }
        }
    }
    let mut out = RgbImage::new(dw, dh);
    let mut acc_row = vec![[0f64; 3]; dwu];
    for (dy, ws) in vw.iter().enumerate() {
        acc_row.fill([0f64; 3]);
        // Row-sequential accumulation: each source row streams once instead
        // of striding column-wise through `tmp`.
        for &(si, w) in ws {
            let row = &tmp[si * dwu..(si + 1) * dwu];
            for (acc, t) in acc_row.iter_mut().zip(row) {
                acc[0] += t[0] * w;
                acc[1] += t[1] * w;
                acc[2] += t[2] * w;
            }
        }
        let out_row = &mut (*out)[dy * dwu * 3..(dy + 1) * dwu * 3];
        for (px, acc) in out_row.chunks_exact_mut(3).zip(&acc_row) {
            px[0] = round_u8(acc[0]);
            px[1] = round_u8(acc[1]);
            px[2] = round_u8(acc[2]);
        }
    }
    out
}

fn round_u8(v: f64) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

// ---------------------------------------------------------------------------
// Pixel-exact reimplementation of Pillow's `Image.resize` for 8-bit RGB —
// the kernels docling's layout input passes through (`get_page_image`'s
// default-BICUBIC downsample, then the RT-DETR processor's BILINEAR stretch
// to 640×640). Ported from Pillow `src/libImaging/Resample.c`: per-axis
// coefficient tables quantized to fixed point (`PRECISION_BITS`), a
// horizontal pass then a vertical pass, each rounding through uint8 — that
// intermediate rounding is why a float resampler can never match Pillow
// byte-for-byte.

/// Pillow's `PRECISION_BITS` (32 − 8 − 2).
const PIL_PRECISION_BITS: i32 = 22;

/// Pillow filter kernels.
#[derive(Clone, Copy)]
pub enum PilFilter {
    /// `Image.Resampling.BILINEAR` — triangle, support 1.
    Bilinear,
    /// `Image.Resampling.BICUBIC` — Catmull-Rom-style cubic, a = −0.5,
    /// support 2 (Pillow's — and PIL `resize`'s **default** — kernel).
    Bicubic,
}

impl PilFilter {
    fn support(self) -> f64 {
        match self {
            Self::Bilinear => 1.0,
            Self::Bicubic => 2.0,
        }
    }

    fn eval(self, x: f64) -> f64 {
        match self {
            Self::Bilinear => {
                let x = x.abs();
                if x < 1.0 {
                    1.0 - x
                } else {
                    0.0
                }
            }
            Self::Bicubic => {
                const A: f64 = -0.5;
                let x = x.abs();
                if x < 1.0 {
                    ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0
                } else if x < 2.0 {
                    (((x - 5.0) * x + 8.0) * x - 4.0) * A
                } else {
                    0.0
                }
            }
        }
    }
}

/// Pillow `precompute_coeffs` + `normalize_coeffs_8bpc`: for each output
/// index, the first source index and the fixed-point kernel weights.
fn pil_coeffs(in_size: usize, out_size: usize, filter: PilFilter) -> Vec<(usize, Vec<i32>)> {
    let scale = in_size as f64 / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = filter.support() * filterscale;
    let ss = 1.0 / filterscale;
    (0..out_size)
        .map(|xx| {
            let center = (xx as f64 + 0.5) * scale;
            let xmin = ((center - support + 0.5) as i64).max(0) as usize;
            let xmax = (((center + support + 0.5) as i64).min(in_size as i64) as usize) - xmin;
            let mut k: Vec<f64> = (0..xmax)
                .map(|x| filter.eval(((x + xmin) as f64 - center + 0.5) * ss))
                .collect();
            let ww: f64 = k.iter().sum();
            if ww != 0.0 {
                for w in &mut k {
                    *w /= ww;
                }
            }
            // Pillow's 8-bit quantization: round half away from zero via
            // `(int)(±0.5 + w · 2^PRECISION_BITS)` (C truncation toward zero).
            let quant: Vec<i32> = k
                .iter()
                .map(|&w| {
                    let s = w * f64::from(1i32 << PIL_PRECISION_BITS);
                    if s < 0.0 {
                        (s - 0.5) as i32
                    } else {
                        (s + 0.5) as i32
                    }
                })
                .collect();
            (xmin, quant)
        })
        .collect()
}

/// Pillow `clip8`: shift out the fixed point and clamp (negative sums —
/// possible with the bicubic kernel's negative lobes — clip to 0).
fn pil_clip8(v: i32) -> u8 {
    (v >> PIL_PRECISION_BITS).clamp(0, 255) as u8
}

/// `PIL.Image.resize((dw, dh), resample=filter)` for RGB, byte-exact:
/// horizontal pass then vertical pass, uint8 in between, i32 accumulators
/// seeded with the rounding bias (Pillow `ImagingResampleHorizontal_8bpc`).
pub fn pil_resize(src: &RgbImage, dw: u32, dh: u32, filter: PilFilter) -> RgbImage {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let (dwu, dhu) = (dw as usize, dh as usize);
    let bias = 1i32 << (PIL_PRECISION_BITS - 1);
    // Both passes work on the raw byte rows rather than through
    // `get_pixel`/`put_pixel`: the per-pixel accessors bounds-check and
    // re-index for every tap, and the vertical pass walked *columns*, so a
    // 4-tap bicubic over a 900×1200 page render cost ~30 ms per page on the
    // pipeline's single render thread — slower than the SIMD 3×→2× downscale
    // of a larger image. The arithmetic is unchanged and purely integer
    // (i32 accumulators, no rounding until `pil_clip8`), so any evaluation
    // order gives the same bytes; the Pillow reference hashes below hold.

    // Horizontal pass (skipped when the width is unchanged, like Pillow).
    let hpass: RgbImage = if dwu != sw {
        let coeffs = pil_coeffs(sw, dwu, filter);
        let src_raw = src.as_raw();
        let (sstride, dstride) = (sw * 3, dwu * 3);
        let mut out = vec![0u8; dstride * sh];
        for (row, orow) in src_raw
            .chunks_exact(sstride)
            .zip(out.chunks_exact_mut(dstride))
        {
            for ((xmin, k), o) in coeffs.iter().zip(orow.chunks_exact_mut(3)) {
                let mut acc = [bias; 3];
                let taps = &row[xmin * 3..(xmin + k.len()) * 3];
                for (px, &w) in taps.chunks_exact(3).zip(k) {
                    acc[0] += i32::from(px[0]) * w;
                    acc[1] += i32::from(px[1]) * w;
                    acc[2] += i32::from(px[2]) * w;
                }
                o[0] = pil_clip8(acc[0]);
                o[1] = pil_clip8(acc[1]);
                o[2] = pil_clip8(acc[2]);
            }
        }
        RgbImage::from_raw(dw, sh as u32, out).expect("hpass buffer sized dw×sh×3")
    } else {
        src.clone()
    };

    // Vertical pass: one i32 accumulator row, each source row added in as a
    // whole (an axpy the compiler vectorizes), then clipped out.
    if dhu == sh {
        return hpass;
    }
    let coeffs = pil_coeffs(sh, dhu, filter);
    let hraw = hpass.as_raw();
    let stride = dwu * 3;
    let mut out = vec![0u8; stride * dhu];
    let mut acc = vec![0i32; stride];
    for ((ymin, k), orow) in coeffs.iter().zip(out.chunks_exact_mut(stride)) {
        acc.fill(bias);
        for (y, &w) in k.iter().enumerate() {
            let row = &hraw[(ymin + y) * stride..(ymin + y + 1) * stride];
            for (a, &p) in acc.iter_mut().zip(row) {
                *a += i32::from(p) * w;
            }
        }
        for (o, &a) in orow.iter_mut().zip(&acc) {
            *o = pil_clip8(a);
        }
    }
    RgbImage::from_raw(dw, dh, out).expect("vpass buffer sized dw×dh×3")
}

#[cfg(test)]
mod pil_tests {
    use super::*;
    use image::Rgb;

    /// Deterministic test image — the same LCG generates the Python-side
    /// reference (see the hash constants' provenance below).
    fn lcg_image(w: u32, h: u32) -> RgbImage {
        let mut state = 0x2545f491u64;
        let mut next = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u8
        };
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.put_pixel(x, y, Rgb([next(), next(), next()]));
            }
        }
        img
    }

    fn fnv1a(bytes: &[u8]) -> u64 {
        let mut h = 0xcbf29ce484222325u64;
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// Byte-exactness against Pillow 12.3 (`Image.resize`), reference hashes
    /// generated with the identical LCG image:
    /// down+up, both kernels, odd sizes to exercise the coefficient edges.
    #[test]
    fn matches_pillow_reference_hashes() {
        let img = lcg_image(61, 47);
        for (dw, dh, filter, want) in [
            (40u32, 30u32, PilFilter::Bilinear, PIL_HASH_BILINEAR_DOWN),
            (97, 83, PilFilter::Bilinear, PIL_HASH_BILINEAR_UP),
            (40, 30, PilFilter::Bicubic, PIL_HASH_BICUBIC_DOWN),
            (97, 83, PilFilter::Bicubic, PIL_HASH_BICUBIC_UP),
            (640, 640, PilFilter::Bilinear, PIL_HASH_BILINEAR_640),
        ] {
            let out = pil_resize(&img, dw, dh, filter);
            assert_eq!(
                fnv1a(out.as_raw()),
                want,
                "PIL mismatch at {dw}x{dh} {:?}",
                match filter {
                    PilFilter::Bilinear => "bilinear",
                    PilFilter::Bicubic => "bicubic",
                }
            );
        }
    }

    // Generated by scripts/conformance/gen_pil_resample_ref.py (Pillow 12.3.0).
    const PIL_HASH_BILINEAR_DOWN: u64 = 0x2ac8262283746b4c;
    const PIL_HASH_BILINEAR_UP: u64 = 0x031c9b4dae3ce142;
    const PIL_HASH_BICUBIC_DOWN: u64 = 0xb450da21946e06c3;
    const PIL_HASH_BICUBIC_UP: u64 = 0xc3134a9cff63718d;
    const PIL_HASH_BILINEAR_640: u64 = 0x967d65f732845b9f;
}
