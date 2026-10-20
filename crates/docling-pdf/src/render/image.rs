//! Image XObjects and inline images → premultiplied RGBA pixmaps
//! (ISO 32000-1, 8.9): every bit depth, the colour spaces of [`super::color`],
//! `/Decode` ranges, stencil masks painted in the fill colour, `/SMask` soft
//! masks, stencil and colour-key `/Mask`s. The sample data comes through the
//! raster's filter chain, its libjpeg-exact JPEG decoder and its CCITT
//! decoder, JPEG 2000 through [`super::jpx`] (#598) and JBIG2 through
//! [`super::jbig2`] (#811). An image no decoder can read comes out as a
//! mid-gray block (the picture the layout model should see is "a
//! picture") — a stencil mask too, which used to draw nothing at all.
//!
//! Loading ([`load`]: filters, codec, colour space, alpha plane) is separate
//! from rasterizing ([`rasterize`]: the RGBA pixmap for one destination
//! size) so a document-wide cache can decode a scan once and blit it at the
//! pipeline's several scales. Like docling-parse (`build_bitmap_image`), an
//! image several times larger than its destination is box-averaged down by
//! an integer factor before the blit, so a 5000-pixel-wide scan drawn 600
//! pixels wide costs 600 columns.

use std::collections::HashMap;

use lopdf::{Dictionary, Document, Object};
use tiny_skia::Pixmap;

use super::color::{to_u8, CmykCache, ColorSpace};
use super::objects::{as_stream, get2, get_bool, get_bool2, get_int, get_int2, nums};
use super::{jbig2, jpx};
use crate::raster::{fax, filters, jpeg};

pub struct DecodedImage {
    pub pixmap: Pixmap,
    /// Source size before any reduction (the unit square maps to it).
    pub src_width: u32,
    pub src_height: u32,
}

/// Raw samples: rows of `bpc`-bit components, byte-aligned per row.
struct Samples {
    width: usize,
    height: usize,
    bpc: u32,
    ncomp: usize,
    data: Vec<u8>,
    stride: usize,
    /// A four-component JPEG with an Adobe APP14 marker.
    adobe: bool,
    /// An opacity channel the codec carried inside the image data (a JPX
    /// codestream's alpha under `/SMaskInData`, #598), at the image size.
    inline_alpha: Option<Vec<u8>>,
}

impl Samples {
    fn get(&self, x: usize, y: usize, c: usize) -> u32 {
        let row = &self.data[y * self.stride..];
        match self.bpc {
            8 => u32::from(*row.get(x * self.ncomp + c).unwrap_or(&0)),
            16 => {
                let i = (x * self.ncomp + c) * 2;
                (u32::from(*row.get(i).unwrap_or(&0)) << 8)
                    | u32::from(*row.get(i + 1).unwrap_or(&0))
            }
            bpc => {
                let bit = (x * self.ncomp + c) * bpc as usize;
                let byte = *row.get(bit / 8).unwrap_or(&0);
                u32::from((byte >> (8 - bpc as usize - bit % 8)) & ((1u8 << bpc) - 1))
            }
        }
    }

    fn max(&self) -> f64 {
        ((1u64 << self.bpc) - 1) as f64
    }
}

enum Loaded {
    Samples(Samples),
    Placeholder,
}

/// An image decoded to samples, with everything the rasterizer needs.
pub struct LoadedImage {
    width: usize,
    height: usize,
    is_mask: bool,
    decode_arr: Option<Vec<f64>>,
    samples: Loaded,
    /// The colour space the samples are in (after the CMYK JPEG rule).
    cs: Option<ColorSpace>,
    /// Alpha at source resolution: soft mask, stencil `/Mask`, colour key.
    alpha: Option<Vec<u8>>,
}

impl LoadedImage {
    /// The memory the samples take (for a cache budget).
    pub fn bytes(&self) -> usize {
        let s = match &self.samples {
            Loaded::Samples(s) => s.data.len(),
            Loaded::Placeholder => 0,
        };
        s + self.alpha.as_ref().map_or(0, Vec::len)
    }
}

/// Decode `stream` to samples: the filter chain and codec, the colour
/// space, the alpha plane.
/// The `/Width` × `/Height` an image dictionary declares (the codec's own
/// size wins once decoded; this is what the reduction rule is asked with).
pub fn declared_size(doc: &Document, d: &Dictionary) -> Option<(i64, i64)> {
    Some((
        get_int2(doc, d, b"Width", b"W")?,
        get_int2(doc, d, b"Height", b"H")?,
    ))
}

/// docling-parse's `codec_reduction_shift`: how many halvings a JPEG may be
/// decoded at (libjpeg's 1/2, 1/4, 1/8 reduced inverse DCT) and still hold
/// at least the pixels it is drawn onto at `target_pixels_per_unit` — the
/// renderer's `bitmap_target_pixels_per_unit`, docling's `render_scale` of
/// 1.0 — so the rasterizer keeps minifying and never magnifies samples it
/// once had. A 300 dpi scan drawn full-page decodes at a quarter for the
/// 72 dpi hint and is blitted *up* onto the scale-2 canvas; matching that is
/// what lines the Rust render of a scan up with the shim's. Capped at three
/// halvings and never below 64 samples a side (a thumbnail is not worth the
/// resampling risk); `drawn_*_units` is the axis-aligned extent of the drawn
/// quad in PDF units.
pub fn codec_reduction_shift(
    drawn_width_units: f64,
    drawn_height_units: f64,
    source_width: i64,
    source_height: i64,
    target_pixels_per_unit: f64,
) -> u32 {
    const MAX_SHIFT: u32 = 3;
    const MIN_SOURCE_EXTENT: i64 = 64;
    if target_pixels_per_unit <= 0.0
        || source_width <= 0
        || source_height <= 0
        || drawn_width_units <= 0.0
        || drawn_height_units <= 0.0
    {
        return 0;
    }
    let target_width = drawn_width_units * target_pixels_per_unit;
    let target_height = drawn_height_units * target_pixels_per_unit;
    let mut shift = 0u32;
    while shift < MAX_SHIFT {
        let next_width = source_width >> (shift + 1);
        let next_height = source_height >> (shift + 1);
        if next_width < MIN_SOURCE_EXTENT || next_height < MIN_SOURCE_EXTENT {
            break;
        }
        if (next_width as f64) < target_width || (next_height as f64) < target_height {
            break;
        }
        shift += 1;
    }
    shift
}

/// Whether a reduced decode is allowed at all (docling-parse's
/// `may_reduce_decode`): not for a stencil mask, an image with a soft mask or
/// `/Mask` (its alpha plane is resolved on the full grid), or an Indexed image
/// (its bytes are palette indices — interpolating them means nothing). Only
/// the DCT decoder acts on the shift; every other codec ignores it.
fn may_reduce_decode(doc: &Document, d: &Dictionary, res: Option<&Dictionary>) -> bool {
    if get_bool2(doc, d, b"ImageMask", b"IM").unwrap_or(false) {
        return false;
    }
    if d.has(b"SMask") || d.has(b"Mask") {
        return false;
    }
    !matches!(
        get2(doc, d, b"ColorSpace", b"CS").and_then(|o| ColorSpace::parse(doc, o, res)),
        Some(ColorSpace::Indexed { .. })
    )
}

/// Decode an image XObject (or inline image) into samples. `reduction_shift`
/// asks the JPEG decoder for a `1 / 2^shift` reduced decode when the image
/// allows one ([`codec_reduction_shift`], [`may_reduce_decode`]); the loaded
/// image then reports the reduced size.
pub fn load(
    doc: &Document,
    stream: &lopdf::Stream,
    res: Option<&Dictionary>,
    reduction_shift: u32,
) -> Result<LoadedImage, String> {
    let d = &stream.dict;
    let (width, height) = declared_size(doc, d).ok_or("Width")?;
    if width <= 0
        || height <= 0
        || width > 1 << 16
        || height > 1 << 16
        || width * height > 80_000_000
    {
        return Err("image size".into());
    }
    let is_mask = get_bool2(doc, d, b"ImageMask", b"IM").unwrap_or(false);
    let decode_arr: Option<Vec<f64>> = get2(doc, d, b"Decode", b"D").and_then(|o| nums(doc, o));
    let shift = if reduction_shift > 0 && may_reduce_decode(doc, d, res) {
        reduction_shift
    } else {
        0
    };
    let mut samples = load_samples(doc, stream, res, is_mask, shift)?;
    // The decoded samples' size is the image's (a JPEG's own header, or the
    // reduced decode, wins over the dictionary's `/Width` × `/Height`).
    let (w, h) = match &samples {
        Loaded::Samples(s) => (s.width, s.height),
        Loaded::Placeholder => (width as usize, height as usize),
    };
    if is_mask {
        return Ok(LoadedImage {
            width: w,
            height: h,
            is_mask,
            decode_arr,
            samples,
            cs: None,
            alpha: None,
        });
    }
    let (cs, alpha) = match &mut samples {
        Loaded::Placeholder => (None, None),
        Loaded::Samples(s) => {
            let cs_obj = get2(doc, d, b"ColorSpace", b"CS");
            let guess = || match s.ncomp {
                3 => ColorSpace::DeviceRGB,
                4 => ColorSpace::DeviceCMYK,
                _ => ColorSpace::DeviceGray,
            };
            let cs = match cs_obj {
                Some(o) => ColorSpace::parse(doc, o, res).unwrap_or_else(guess),
                None => guess(),
            };
            // A four-component JPEG under a colour space that is not CMYK:
            // the dictionary contradicts its data, and only the Adobe marker
            // is left to say what the samples hold — 255 minus the ink
            // (docling-parse's rule; a /DeviceCMYK image's samples are ink
            // amounts, and /Decode says otherwise).
            let cs = if s.ncomp == 4 && cs.components() != 4 {
                if s.adobe {
                    for v in &mut s.data {
                        *v = 255 - *v;
                    }
                }
                ColorSpace::DeviceCMYK
            } else {
                cs
            };
            // `/SMask` / `/Mask` first; else the alpha the codec carried
            // inside the data (a JPX codestream's, under `/SMaskInData`).
            let alpha = match alpha_plane(doc, d, res, s, w, h)? {
                Some(a) => Some(a),
                None => s.inline_alpha.take(),
            };
            (Some(cs), alpha)
        }
    };
    Ok(LoadedImage {
        width: w,
        height: h,
        is_mask,
        decode_arr,
        samples,
        cs,
        alpha,
    })
}

/// Rasterize a loaded image as it would be painted with `fill` (for stencil
/// masks), reduced so that its whole extent covers about `target` device
/// pixels (`None` = full size).
pub fn rasterize(
    img: &LoadedImage,
    fill: [u8; 3],
    target: Option<(u32, u32)>,
    cmyk: &mut CmykCache,
) -> Result<DecodedImage, String> {
    let (w, h) = (img.width, img.height);
    // Integer reduction factors (docling-parse's `fx = sw / dst_w`).
    let (fx, fy) = match target {
        Some((tw, th)) if tw > 0 && th > 0 => (
            ((w as u32) / tw).max(1) as usize,
            ((h as u32) / th).max(1) as usize,
        ),
        _ => (1, 1),
    };
    let out_w = (w / fx).max(1);
    let out_h = (h / fy).max(1);
    let mut pixmap = Pixmap::new(out_w as u32, out_h as u32).ok_or("pixmap")?;
    let done = |pixmap| {
        Ok(DecodedImage {
            pixmap,
            src_width: w as u32,
            src_height: h as u32,
        })
    };

    if img.is_mask {
        // 1 bpc; sample 0 paints (Decode [0 1]) unless Decode [1 0].
        let paint_on_one = img
            .decode_arr
            .as_ref()
            .is_some_and(|v| v.first().is_some_and(|x| *x >= 0.5));
        match &img.samples {
            Loaded::Samples(s) => {
                let px = pixmap.pixels_mut();
                for oy in 0..out_h {
                    for ox in 0..out_w {
                        let mut cov = 0u32;
                        let mut n = 0u32;
                        for y in oy * fy..((oy + 1) * fy).min(h) {
                            for x in ox * fx..((ox + 1) * fx).min(w) {
                                let v = s.get(x, y, 0);
                                let on = (v != 0) == paint_on_one;
                                cov += u32::from(on);
                                n += 1;
                            }
                        }
                        let a = (cov * 255).checked_div(n).unwrap_or(0) as u8;
                        px[oy * out_w + ox] = premul(fill, a);
                    }
                }
            }
            // An undecodable stencil (#811): the same neutral block an
            // undecodable image gets, so the page shows *something* where
            // the scan's text was instead of nothing at all.
            Loaded::Placeholder => {
                for p in pixmap.pixels_mut() {
                    *p = premul([128, 128, 128], 255);
                }
            }
        }
        return done(pixmap);
    }

    let (Loaded::Samples(s), Some(cs)) = (&img.samples, &img.cs) else {
        // Undecodable (a codec failure): a neutral block the size of the image.
        for p in pixmap.pixels_mut() {
            *p = premul([128, 128, 128], 255);
        }
        return done(pixmap);
    };

    let ncomp = s.ncomp.min(cs.components()).max(1);
    let max = s.max();
    let default_decode = cs.default_decode(s.bpc);
    let decode = match &img.decode_arr {
        Some(v) if v.len() >= 2 * ncomp => v.clone(),
        _ => default_decode.clone(),
    };
    let is_default_decode = decode
        .iter()
        .zip(default_decode.iter())
        .all(|(a, b)| (a - b).abs() < 1e-9);
    let alpha = &img.alpha;

    // Fast paths: 8-bit gray / RGB samples with the default decode and no
    // alpha — the scanned page and the photograph — averaged directly.
    let direct = alpha.is_none()
        && s.bpc == 8
        && is_default_decode
        && matches!(
            (cs, s.ncomp),
            (ColorSpace::DeviceGray, 1) | (ColorSpace::DeviceRGB, 3)
        );
    if direct {
        let px = pixmap.pixels_mut();
        let nc = s.ncomp;
        for oy in 0..out_h {
            for ox in 0..out_w {
                let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
                for y in oy * fy..((oy + 1) * fy).min(h) {
                    let row = &s.data[y * s.stride..];
                    for x in ox * fx..((ox + 1) * fx).min(w) {
                        let i = x * nc;
                        if nc == 1 {
                            let v = u32::from(row[i]);
                            r += v;
                            g += v;
                            b += v;
                        } else {
                            r += u32::from(row[i]);
                            g += u32::from(row[i + 1]);
                            b += u32::from(row[i + 2]);
                        }
                        n += 1;
                    }
                }
                let n = n.max(1);
                px[oy * out_w + ox] = tiny_skia::PremultipliedColorU8::from_rgba(
                    (r / n) as u8,
                    (g / n) as u8,
                    (b / n) as u8,
                    255,
                )
                .unwrap_or(tiny_skia::PremultipliedColorU8::TRANSPARENT);
            }
        }
        return done(pixmap);
    }

    // Sample tuple → RGB, memoised (indexed, gray, CMYK and low depths repeat).
    let mut memo: HashMap<u64, [u8; 3]> = HashMap::new();
    let mut convert = |vals: &[u32]| -> [u8; 3] {
        let key = vals
            .iter()
            .take(4)
            .fold(0u64, |a, &v| (a << 16) | u64::from(v & 0xFFFF));
        if let Some(c) = memo.get(&key) {
            return *c;
        }
        let rgb = match (cs, s.bpc, is_default_decode) {
            (ColorSpace::DeviceRGB, 8, true) => [vals[0] as u8, vals[1] as u8, vals[2] as u8],
            (ColorSpace::DeviceGray, 8, true) => [vals[0] as u8; 3],
            (ColorSpace::DeviceCMYK, 8, true) => {
                cmyk.rgb8(vals[0] as u8, vals[1] as u8, vals[2] as u8, vals[3] as u8)
            }
            _ => {
                let comps: Vec<f64> = (0..ncomp)
                    .map(|i| {
                        let (dmin, dmax) = (decode[2 * i], decode[2 * i + 1]);
                        dmin + f64::from(vals[i]) * (dmax - dmin) / max
                    })
                    .collect();
                match cs.to_rgb(&comps) {
                    Some(c) => [to_u8(c[0]), to_u8(c[1]), to_u8(c[2])],
                    None => [255, 255, 255],
                }
            }
        };
        if memo.len() < 1 << 18 {
            memo.insert(key, rgb);
        }
        rgb
    };

    let px = pixmap.pixels_mut();
    let mut vals = vec![0u32; ncomp.max(4)];
    for oy in 0..out_h {
        for ox in 0..out_w {
            let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for y in oy * fy..((oy + 1) * fy).min(h) {
                for x in ox * fx..((ox + 1) * fx).min(w) {
                    for (c, v) in vals.iter_mut().enumerate().take(ncomp) {
                        *v = s.get(x, y, c);
                    }
                    let rgb = convert(&vals[..ncomp.max(1)]);
                    let al = match alpha {
                        Some(pl) => u32::from(pl[y * w + x]),
                        None => 255,
                    };
                    // Premultiply before averaging, like docling-parse.
                    r += u32::from(rgb[0]) * al / 255;
                    g += u32::from(rgb[1]) * al / 255;
                    b += u32::from(rgb[2]) * al / 255;
                    a += al;
                    n += 1;
                }
            }
            let n = n.max(1);
            px[oy * out_w + ox] = tiny_skia::PremultipliedColorU8::from_rgba(
                (r / n).min(255) as u8,
                (g / n).min(255) as u8,
                (b / n).min(255) as u8,
                (a / n).min(255) as u8,
            )
            .unwrap_or(tiny_skia::PremultipliedColorU8::TRANSPARENT);
        }
    }
    done(pixmap)
}

/// [`load`] then [`rasterize`].
pub fn decode(
    doc: &Document,
    stream: &lopdf::Stream,
    res: Option<&Dictionary>,
    fill: [u8; 3],
    target: Option<(u32, u32)>,
    cmyk: &mut CmykCache,
) -> Result<DecodedImage, String> {
    let img = load(doc, stream, res, 0)?;
    rasterize(&img, fill, target, cmyk)
}

fn premul(rgb: [u8; 3], a: u8) -> tiny_skia::PremultipliedColorU8 {
    let m = |c: u8| ((u32::from(c) * u32::from(a) + 127) / 255) as u8;
    tiny_skia::PremultipliedColorU8::from_rgba(m(rgb[0]), m(rgb[1]), m(rgb[2]), a)
        .unwrap_or(tiny_skia::PremultipliedColorU8::TRANSPARENT)
}

/// Run the filter chain and the image codec; `Placeholder` for an image
/// its codec cannot decode.
fn load_samples(
    doc: &Document,
    stream: &lopdf::Stream,
    res: Option<&Dictionary>,
    is_mask: bool,
    reduction_shift: u32,
) -> Result<Loaded, String> {
    let d = &stream.dict;
    let w = get_int2(doc, d, b"Width", b"W").unwrap_or(0).max(0) as usize;
    let h = get_int2(doc, d, b"Height", b"H").unwrap_or(0).max(0) as usize;
    let chain = filters::filters(doc, d);
    let (data, codec) =
        filters::apply(doc, &stream.content, &chain).map_err(|e| format!("filter {e:?}"))?;
    let bpc_dict = if is_mask {
        1
    } else {
        get_int2(doc, d, b"BitsPerComponent", b"BPC")
            .unwrap_or(8)
            .clamp(1, 16) as u32
    };
    let cs_ncomp = if is_mask {
        1
    } else {
        get2(doc, d, b"ColorSpace", b"CS")
            .and_then(|o| ColorSpace::parse(doc, o, res))
            .map(|c| c.components())
            .unwrap_or(1)
    };
    match codec {
        None => {
            let bpc = match bpc_dict {
                1 | 2 | 4 | 8 | 16 => bpc_dict,
                _ => 8,
            };
            let stride = (w * cs_ncomp * bpc as usize).div_ceil(8);
            let mut data = data;
            if data.len() < stride * h {
                // Short data: pad (a truncated image shows what it has).
                data.resize(stride * h, 0);
            }
            Ok(Loaded::Samples(Samples {
                width: w,
                height: h,
                bpc,
                ncomp: cs_ncomp,
                data,
                stride,
                adobe: false,
                inline_alpha: None,
            }))
        }
        Some(codec) if codec.name == "DCTDecode" => {
            let transform = codec
                .parms
                .as_ref()
                .and_then(|p| get_int(doc, p, b"ColorTransform"))
                .unwrap_or(1)
                != 0;
            match jpeg::decode(&data, transform, 1u32 << reduction_shift.min(3)) {
                Ok(img) => {
                    let ncomp = img.channels;
                    // The JPEG's own size wins over the dictionary's.
                    Ok(Loaded::Samples(Samples {
                        width: img.width,
                        height: img.height,
                        bpc: 8,
                        ncomp,
                        stride: img.width * ncomp,
                        adobe: img.adobe_inverted,
                        inline_alpha: None,
                        data: img.data,
                    }))
                }
                Err(e) => {
                    docling_core::debug_log!("docling-pdf render: JPEG not decoded ({e:?})");
                    Ok(Loaded::Placeholder)
                }
            }
        }
        Some(codec) if codec.name == "CCITTFaxDecode" => {
            let p = codec.parms.as_ref();
            let pi = |k: &[u8], default: i64| p.and_then(|p| get_int(doc, p, k)).unwrap_or(default);
            let pb = |k: &[u8]| p.and_then(|p| get_bool(doc, p, k)).unwrap_or(false);
            let mut rows = pi(b"Rows", 0);
            if rows > i64::from(u16::MAX) {
                rows = 0;
            }
            let params = fax::Params {
                k: pi(b"K", 0) as i32,
                end_of_line: pb(b"EndOfLine"),
                byte_align: pb(b"EncodedByteAlign"),
                black_is_1: pb(b"BlackIs1"),
                columns: pi(b"Columns", 1728).clamp(1, 65535) as usize,
                rows: rows.max(0) as usize,
            };
            let lines = fax::decode(&data, &params, h);
            let stride = w.div_ceil(8);
            let mut out = vec![0u8; stride * h];
            for (y, row) in out.chunks_exact_mut(stride).enumerate() {
                match lines.get(y) {
                    Some(Some(line)) => {
                        let n = stride.min(line.len());
                        row[..n].copy_from_slice(&line[..n]);
                    }
                    // Past the data: white (CCITT 1 = white unless BlackIs1).
                    _ => row.fill(if params.black_is_1 { 0x00 } else { 0xff }),
                }
            }
            Ok(Loaded::Samples(Samples {
                width: w,
                height: h,
                bpc: 1,
                ncomp: 1,
                data: out,
                stride,
                adobe: false,
                inline_alpha: None,
            }))
        }
        Some(codec) if codec.name == "JPXDecode" => {
            // 7.4.9: an `Indexed` colour space makes the samples palette
            // indices, so the codestream's own palette must not be applied;
            // `/SMaskInData` says whether the codestream's alpha is the soft
            // mask (1, or 2 premultiplied) or to be ignored (0, default).
            let indexed = !is_mask
                && matches!(
                    get2(doc, d, b"ColorSpace", b"CS").and_then(|o| ColorSpace::parse(doc, o, res)),
                    Some(ColorSpace::Indexed { .. })
                );
            let smask_in_data = get_int(doc, d, b"SMaskInData").unwrap_or(0);
            let target = (reduction_shift > 0 && w > 0 && h > 0).then(|| {
                let shift = reduction_shift.min(3);
                ((w >> shift).max(1) as u32, (h >> shift).max(1) as u32)
            });
            match jpx::decode(&data, indexed, target) {
                Ok(img) => Ok(Loaded::Samples(Samples {
                    width: img.width,
                    height: img.height,
                    bpc: 8,
                    ncomp: img.ncomp,
                    stride: img.width * img.ncomp,
                    adobe: false,
                    inline_alpha: if smask_in_data > 0 { img.alpha } else { None },
                    data: img.data,
                })),
                Err(e) => {
                    docling_core::debug_log!("docling-pdf render: JPX not decoded ({e})");
                    Ok(Loaded::Placeholder)
                }
            }
        }
        Some(codec) if codec.name == "JBIG2Decode" => {
            // 7.4.7: `/JBIG2Globals` is a stream of segments shared by the
            // document's images, decoded ahead of this one's.
            let globals = codec
                .parms
                .as_ref()
                .and_then(|p| p.get(b"JBIG2Globals").ok())
                .and_then(|o| as_stream(doc, o))
                .and_then(|g| {
                    let chain = filters::filters(doc, &g.dict);
                    match filters::apply(doc, &g.content, &chain) {
                        Ok((bytes, None)) => Some(bytes),
                        _ => {
                            docling_core::debug_log!(
                                "docling-pdf render: JBIG2Globals not decoded, ignored"
                            );
                            None
                        }
                    }
                });
            match jbig2::decode(&data, globals.as_deref(), w, h) {
                Ok(img) => Ok(Loaded::Samples(Samples {
                    width: w,
                    height: h,
                    bpc: 1,
                    ncomp: 1,
                    stride: img.stride,
                    adobe: false,
                    inline_alpha: None,
                    data: img.data,
                })),
                Err(e) => {
                    docling_core::debug_log!("docling-pdf render: JBIG2 not decoded ({e})");
                    Ok(Loaded::Placeholder)
                }
            }
        }
        Some(codec) => {
            docling_core::debug_log!(
                "docling-pdf render: no {} decoder, drawing a placeholder",
                codec.name
            );
            Ok(Loaded::Placeholder)
        }
    }
}

/// Take a mask's samples (any bit depth) as an alpha plane at the image size:
/// the soft mask's gray, a stencil `/Mask`'s 1 = masked out, or the colour
/// key ranges on the raw samples.
fn alpha_plane(
    doc: &Document,
    d: &Dictionary,
    res: Option<&Dictionary>,
    s: &Samples,
    w: usize,
    h: usize,
) -> Result<Option<Vec<u8>>, String> {
    // Soft mask.
    if let Some(sm) = d.get(b"SMask").ok().and_then(|o| as_stream(doc, o)) {
        let smw = get_int(doc, &sm.dict, b"Width").unwrap_or(0).max(0) as usize;
        let smh = get_int(doc, &sm.dict, b"Height").unwrap_or(0).max(0) as usize;
        if smw > 0 && smh > 0 && smw * smh <= 80_000_000 {
            if let Ok(Loaded::Samples(ms)) = load_samples(doc, sm, res, false, 0) {
                let decode = get2(doc, &sm.dict, b"Decode", b"D").and_then(|o| nums(doc, o));
                let invert = decode.is_some_and(|v| v.first().is_some_and(|x| *x >= 0.5));
                let mmax = ms.max();
                let mut plane = vec![255u8; w * h];
                for y in 0..h {
                    let my = (y * ms.height / h.max(1)).min(ms.height.saturating_sub(1));
                    for x in 0..w {
                        let mx = (x * ms.width / w.max(1)).min(ms.width.saturating_sub(1));
                        let v = f64::from(ms.get(mx, my, 0)) / mmax;
                        let v = if invert { 1.0 - v } else { v };
                        plane[y * w + x] = (v * 255.0).round() as u8;
                    }
                }
                return Ok(Some(plane));
            }
        }
    }
    match d.get(b"Mask").ok().map(|o| super::objects::deref(doc, o)) {
        // Stencil mask: 1 bpc, sample 1 = masked (unless Decode [1 0]).
        Some(Object::Stream(ms)) => {
            let mw = get_int(doc, &ms.dict, b"Width").unwrap_or(0).max(0) as usize;
            let mh = get_int(doc, &ms.dict, b"Height").unwrap_or(0).max(0) as usize;
            if mw > 0 && mh > 0 && mw * mh <= 80_000_000 {
                if let Ok(Loaded::Samples(bits)) = load_samples(doc, ms, res, true, 0) {
                    let decode = get2(doc, &ms.dict, b"Decode", b"D").and_then(|o| nums(doc, o));
                    let one_paints = decode.is_some_and(|v| v.first().is_some_and(|x| *x >= 0.5));
                    let mut plane = vec![255u8; w * h];
                    for y in 0..h {
                        let my = (y * bits.height / h.max(1)).min(bits.height.saturating_sub(1));
                        for x in 0..w {
                            let mx = (x * bits.width / w.max(1)).min(bits.width.saturating_sub(1));
                            let v = bits.get(mx, my, 0) != 0;
                            let masked = if one_paints { !v } else { v };
                            if masked {
                                plane[y * w + x] = 0;
                            }
                        }
                    }
                    return Ok(Some(plane));
                }
            }
            Ok(None)
        }
        // Colour key: ranges per component on the raw integer samples.
        Some(Object::Array(ranges)) => {
            let r: Vec<i64> = ranges
                .iter()
                .filter_map(|o| {
                    super::objects::num(super::objects::deref(doc, o)).map(|v| v as i64)
                })
                .collect();
            let n = s.ncomp;
            if r.len() < 2 * n {
                return Ok(None);
            }
            let mut plane = vec![255u8; w * h];
            for y in 0..h.min(s.height) {
                for x in 0..w.min(s.width) {
                    let masked = (0..n).all(|c| {
                        let v = i64::from(s.get(x, y, c));
                        v >= r[2 * c] && v <= r[2 * c + 1]
                    });
                    if masked {
                        plane[y * w + x] = 0;
                    }
                }
            }
            Ok(Some(plane))
        }
        _ => Ok(None),
    }
}
