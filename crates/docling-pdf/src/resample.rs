//! Pixel-exact reimplementations of the OpenCV resize kernels docling uses for
//! TableFormer preprocessing, so the model sees byte-identical input. Verified
//! against cv2 on docling's own bitmaps (INTER_AREA max diff 1/255, INTER_LINEAR
//! < 1e-4 in float).

use image::RgbImage;

/// Per-output-pixel source spans + overlap weights for area resampling:
/// `(first source index, weights)`, the taps being the contiguous run of
/// source pixels the output pixel covers, in increasing index order.
fn area_weights(src: usize, dst: usize, scale: f64) -> Vec<(usize, Vec<f64>)> {
    (0..dst)
        .map(|d| {
            let f1 = d as f64 * scale;
            let f2 = (d + 1) as f64 * scale;
            let s1 = f1.floor() as usize;
            let s2 = (f2.ceil() as usize).min(src);
            let ws = (s1..s2)
                .map(|si| (((si + 1) as f64).min(f2) - (si as f64).max(f1)) / scale)
                .collect();
            (s1, ws)
        })
        .collect()
}

/// `cv2.resize(..., interpolation=INTER_AREA)` for shrinking — area-weighted
/// averaging, separable (horizontal then vertical), f64 accumulation.
///
/// The per-pixel addition order is the naive form's — horizontal taps in
/// increasing source column, then vertical taps in increasing source row — so
/// the f64 sums, and the rounded bytes, are bit-identical to it (asserted by
/// `area_tests`). Within that contract the work is arranged for the cache:
/// a horizontally-shrunk source row is computed on demand as the vertical
/// pass reaches it and kept only while an output row still needs it (a
/// source row feeds at most two output rows, so a ring of a few `f64` rows
/// replaces the 30 MB `sh × dw` intermediate a full first pass wrote and
/// re-read), the horizontal taps run over the contiguous byte span they
/// cover (no per-tap indexing), and the vertical pass is a flat `f64` axpy
/// the compiler vectorizes. ~3× faster than the two-pass form on a page
/// render (43 → 18 ms, 1224×1584 → 791×1024, release, one thread).
pub fn inter_area(src: &RgbImage, dw: u32, dh: u32) -> RgbImage {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let (dwu, dhu) = (dw as usize, dh as usize);
    let hw = area_weights(sw, dwu, sw as f64 / dw as f64);
    let vw = area_weights(sh, dhu, sh as f64 / dh as f64);
    let raw = src.as_raw();
    let stride = dwu * 3;

    // Horizontal shrink of one source row into `dst` (dw × 3 f64).
    let shrink_row = |sy: usize, dst: &mut [f64]| {
        let src_row = &raw[sy * sw * 3..(sy + 1) * sw * 3];
        for ((s1, ws), acc) in hw.iter().zip(dst.chunks_exact_mut(3)) {
            let taps = &src_row[s1 * 3..(s1 + ws.len()) * 3];
            let mut a = [0f64; 3];
            for (p, &w) in taps.chunks_exact(3).zip(ws) {
                a[0] += f64::from(p[0]) * w;
                a[1] += f64::from(p[1]) * w;
                a[2] += f64::from(p[2]) * w;
            }
            acc.copy_from_slice(&a);
        }
    };

    // Ring of shrunk source rows keyed by source row index. Output rows walk
    // the source monotonically, so a row older than the current window's
    // first tap is never needed again and its buffer is recycled.
    let mut ring: Vec<(usize, Vec<f64>)> = Vec::new();
    let mut spare: Vec<Vec<f64>> = Vec::new();
    let mut out = vec![0u8; stride * dhu];
    let mut acc = vec![0f64; stride];
    for ((s1, ws), out_row) in vw.iter().zip(out.chunks_exact_mut(stride)) {
        let mut i = 0;
        while i < ring.len() {
            if ring[i].0 < *s1 {
                spare.push(ring.swap_remove(i).1);
            } else {
                i += 1;
            }
        }
        acc.fill(0.0);
        for (k, &w) in ws.iter().enumerate() {
            let sy = s1 + k;
            let row = match ring.iter().position(|(y, _)| *y == sy) {
                Some(j) => &ring[j].1,
                None => {
                    let mut buf = spare.pop().unwrap_or_else(|| vec![0f64; stride]);
                    shrink_row(sy, &mut buf);
                    ring.push((sy, buf));
                    &ring[ring.len() - 1].1
                }
            };
            for (a, t) in acc.iter_mut().zip(row) {
                *a += t * w;
            }
        }
        for (o, &a) in out_row.iter_mut().zip(&acc) {
            *o = round_u8(a);
        }
    }
    RgbImage::from_raw(dw, dh, out).expect("inter_area buffer sized dw×dh×3")
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

#[cfg(test)]
mod area_tests {
    use super::*;

    fn lcg_image(w: u32, h: u32, seed: u64) -> RgbImage {
        let mut state = seed;
        let mut next = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u8
        };
        let mut raw = vec![0u8; (w * h * 3) as usize];
        for b in &mut raw {
            *b = next();
        }
        RgbImage::from_raw(w, h, raw).unwrap()
    }

    /// Reference: the naive per-output-pixel form, taps in increasing source
    /// index, horizontal then vertical — the addition order the fast path
    /// must reproduce for bit-identical bytes.
    fn inter_area_naive(src: &RgbImage, dw: u32, dh: u32) -> RgbImage {
        let (sw, sh) = (src.width() as usize, src.height() as usize);
        let hw = area_weights(sw, dw as usize, sw as f64 / dw as f64);
        let vw = area_weights(sh, dh as usize, sh as f64 / dh as f64);
        let mut out = RgbImage::new(dw, dh);
        for (dy, vws) in vw.iter().enumerate() {
            for (dx, hws) in hw.iter().enumerate() {
                let mut acc = [0f64; 3];
                for (ky, &wy) in vws.1.iter().enumerate() {
                    let sy = vws.0 + ky;
                    let mut t = [0f64; 3];
                    for (kx, &wx) in hws.1.iter().enumerate() {
                        let sx = hws.0 + kx;
                        let p = src.get_pixel(sx as u32, sy as u32).0;
                        for c in 0..3 {
                            t[c] += p[c] as f64 * wx;
                        }
                    }
                    for c in 0..3 {
                        acc[c] += t[c] * wy;
                    }
                }
                out.put_pixel(
                    dx as u32,
                    dy as u32,
                    image::Rgb([round_u8(acc[0]), round_u8(acc[1]), round_u8(acc[2])]),
                );
            }
        }
        out
    }

    #[test]
    fn inter_area_matches_naive_order() {
        for (i, (sw, sh, dw, dh)) in [
            (1224u32, 1584u32, 791u32, 1024u32),
            (1190, 1684, 723, 1024),
            (1584, 1224, 1325, 1024),
            (61, 47, 40, 30),
            (100, 100, 100, 50),
            (37, 91, 36, 90),
        ]
        .into_iter()
        .enumerate()
        {
            let img = lcg_image(sw, sh, 0x9e3779b97f4a7c15 ^ i as u64);
            assert_eq!(
                inter_area(&img, dw, dh).as_raw(),
                inter_area_naive(&img, dw, dh).as_raw(),
                "{sw}x{sh} -> {dw}x{dh}"
            );
        }
    }

    #[test]
    #[ignore = "timing only: cargo test --release -p docling-pdf --lib area_tests::bench -- --ignored --nocapture"]
    fn bench_inter_area() {
        let img = lcg_image(1224, 1584, 7);
        let _ = inter_area(&img, 791, 1024);
        let t = std::time::Instant::now();
        let n = 20;
        for _ in 0..n {
            std::hint::black_box(inter_area(&img, 791, 1024));
        }
        eprintln!(
            "inter_area 1224x1584 -> 791x1024: {:.1} ms",
            t.elapsed().as_secs_f64() * 1e3 / n as f64
        );
    }
}
