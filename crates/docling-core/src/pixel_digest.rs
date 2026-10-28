//! Content digest of a picture's pixels, as docling names exported image
//! assets.
//!
//! docling-core's `PictureItem._image_to_hexhash` hashes `PIL img.tobytes()` —
//! the *decoded* pixel buffer in the image's PIL mode — not the encoded file,
//! so the `image_{NNNNNN}_{sha256}.png` names of a referenced-image export are
//! independent of how the PNG was compressed. That buffer is reproduced here
//! for the two encodings office documents embed:
//!
//! - **PNG**: Pillow's `PngImagePlugin` maps every (bit depth, colour type)
//!   pair to one mode/rawmode (`_MODES`), and the backends' PNG round-trip
//!   (`save(format="PNG")` + reopen) keeps that mode; [`pil_png_bytes`]
//!   rebuilds the bytes from the raw (untransformed) samples.
//! - **JPEG**: Pillow decodes through libjpeg(-turbo) at its defaults, which
//!   [`crate::jpeg`] reproduces byte for byte (gray → `L`, otherwise `RGB`;
//!   CMYK is left out — PIL cannot write it as PNG, so docling's office
//!   backends never keep such an image).
//!
//! Anything else (GIF, BMP, EMF, …) keeps the encoded-bytes digest.
//!
//! Decoding every picture for its digest is the cost of a referenced-image
//! export (#898: 4 s for 1359 pictures, on one thread, against 0.2 s to
//! write the archive), so [`Assets`] decodes each distinct image once — the
//! digest and the PNG bytes from one decode — across the host's cores,
//! before the serializer walks the document.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

/// Lowercase hex SHA-256 of `bytes`.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The asset digest of an embedded image: docling's pixel hash when the
/// image is a PNG or JPEG we can decode, else the hash of the encoded bytes.
pub(crate) fn image_digest(data: &[u8]) -> String {
    match pil_bytes(data) {
        Some(px) => sha256_hex(&px),
        None => sha256_hex(data),
    }
}

/// `PIL.Image.open(data).tobytes()` for a PNG or (gray/RGB) JPEG.
fn pil_bytes(data: &[u8]) -> Option<Vec<u8>> {
    if is_png(data) {
        pil_png_bytes(data)
    } else {
        decode_jpeg(data).map(|j| j.data)
    }
}

fn is_png(data: &[u8]) -> bool {
    data.starts_with(b"\x89PNG\r\n\x1a\n")
}

/// A gray or RGB JPEG decoded like libjpeg (CMYK and anything the decoder
/// does not support → `None`).
fn decode_jpeg(data: &[u8]) -> Option<crate::jpeg::Image> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    crate::jpeg::decode(data, true, 1)
        .ok()
        .filter(|j| matches!(j.channels, 1 | 3))
}

/// The image as a PNG file — what an archive stores under the asset's
/// `.png` name (docling writes every picture asset as PNG): a PNG source
/// as-is, a gray/RGB JPEG re-encoded from its libjpeg pixels (the same ones
/// the digest covers). `None` for other encodings, which the caller converts
/// with a general-purpose decoder if it has one.
pub(crate) fn asset_png(data: &[u8]) -> Option<Vec<u8>> {
    if is_png(data) {
        return Some(data.to_vec());
    }
    encode_png(&decode_jpeg(data)?)
}

/// [`image_digest`] and [`asset_png`] of one image from one decode (a JPEG
/// is decoded once, not twice).
pub(crate) fn asset_of(data: &[u8]) -> (String, Option<Vec<u8>>) {
    if is_png(data) {
        let digest = match pil_png_bytes(data) {
            Some(px) => sha256_hex(&px),
            None => sha256_hex(data),
        };
        return (digest, Some(data.to_vec()));
    }
    match decode_jpeg(data) {
        Some(j) => (sha256_hex(&j.data), encode_png(&j)),
        None => (sha256_hex(data), None),
    }
}

/// The digests and PNG bytes of a document's pictures, computed ahead of a
/// referenced-image export (#898): each distinct image — keyed by its bytes'
/// address, stable for the document being exported — decoded once, in
/// parallel on the host's cores. The serializers take the PNG out as they
/// name the asset; an image the cache does not hold (or holds no longer) is
/// decoded on the spot, so the output is the same either way.
pub(crate) struct Assets {
    map: HashMap<AssetKey, (String, Option<Vec<u8>>)>,
}

/// An image's bytes by address and length.
type AssetKey = (usize, usize);

impl Assets {
    /// Nothing prepared: every lookup decodes.
    pub(crate) fn empty() -> Self {
        Assets {
            map: HashMap::new(),
        }
    }

    /// Decode and digest every distinct image in `images`.
    pub(crate) fn prepare<'a>(images: impl IntoIterator<Item = &'a [u8]>) -> Self {
        let mut keys = Vec::new();
        let mut distinct: Vec<&[u8]> = Vec::new();
        for data in images {
            let key = Self::key(data);
            if !keys.contains(&key) {
                keys.push(key);
                distinct.push(data);
            }
        }
        let assets = parallel_map(&distinct, |data| asset_of(data));
        Assets {
            map: keys.into_iter().zip(assets).collect(),
        }
    }

    fn key(data: &[u8]) -> AssetKey {
        (data.as_ptr() as usize, data.len())
    }

    /// [`image_digest`] of `data`.
    pub(crate) fn digest(&self, data: &[u8]) -> String {
        match self.map.get(&Self::key(data)) {
            Some((digest, _)) => digest.clone(),
            None => image_digest(data),
        }
    }

    /// [`asset_png`] of `data`, or its own bytes for an encoding it does
    /// not cover — moved out of the cache on the first call (a second asset
    /// of the same image re-encodes).
    pub(crate) fn take_png(&mut self, data: &[u8]) -> Vec<u8> {
        self.map
            .get_mut(&Self::key(data))
            .and_then(|(_, png)| png.take())
            .or_else(|| asset_png(data))
            .unwrap_or_else(|| data.to_vec())
    }
}

/// `f` over every item, on up to one thread per core (the host's
/// parallelism, at most one thread per item), results in item order. Serial
/// for one item, on wasm32 (no threads), or when a thread cannot be spawned.
fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(items.len());
        if threads > 1 {
            let f = &f;
            let mut slots: Vec<Option<R>> = items.iter().map(|_| None).collect();
            let done = std::thread::scope(|scope| {
                let mut handles = Vec::with_capacity(threads);
                for t in 0..threads {
                    // Strided, so a run of large images spreads across the
                    // threads instead of landing on one.
                    let worker = move || -> Vec<(usize, R)> {
                        items
                            .iter()
                            .enumerate()
                            .skip(t)
                            .step_by(threads)
                            .map(|(i, item)| (i, f(item)))
                            .collect()
                    };
                    match std::thread::Builder::new()
                        .name("docling-assets".into())
                        .spawn_scoped(scope, worker)
                    {
                        Ok(h) => handles.push(h),
                        Err(_) => return false,
                    }
                }
                for h in handles {
                    let Ok(part) = h.join() else {
                        return false;
                    };
                    for (i, r) in part {
                        slots[i] = Some(r);
                    }
                }
                true
            });
            if done && slots.iter().all(Option::is_some) {
                return slots.into_iter().map(|s| s.unwrap()).collect();
            }
        }
    }
    items.iter().map(f).collect()
}

/// The PNG file of a decoded gray/RGB JPEG.
fn encode_png(j: &crate::jpeg::Image) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, j.width as u32, j.height as u32);
        enc.set_color(if j.channels == 1 {
            png::ColorType::Grayscale
        } else {
            png::ColorType::Rgb
        });
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(&j.data).ok()?;
    }
    Some(buf)
}

/// The bytes `PIL.Image.open(png).tobytes()` returns for a PNG.
///
/// Pillow's `_MODES` table, per (bit depth, colour type):
/// gray 1 → `1` (rows bit-packed MSB-first, byte-padded: the PNG scanline
/// as-is); gray 2/4 → `L` (unpacked, scaled ×0x55 / ×0x11); gray 16 → `I;16`
/// (little-endian); RGB/RGBA/LA 16 → 8-bit `RGB`/`RGBA` (the MSB of each
/// sample) — 16-bit gray+alpha becomes `RGBA` (`LA;16B`: L copied to R, G,
/// B); palette 1/2/4/8 → `P` (one index byte per pixel). `tRNS` only sets
/// `info["transparency"]`, never the mode, so it does not enter the bytes.
fn pil_png_bytes(data: &[u8]) -> Option<Vec<u8>> {
    use png::{BitDepth, ColorType};
    let mut dec = png::Decoder::new(std::io::Cursor::new(data));
    dec.set_transformations(png::Transformations::IDENTITY);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let line = info.line_size;
    let rows = buf.chunks_exact(line).take(h);
    let depth = info.bit_depth as u8;
    let out = match (info.color_type, info.bit_depth) {
        // `1`: the packed scanlines are PIL's packed rows.
        (ColorType::Grayscale, BitDepth::One) => rows.flat_map(|r| r.iter().copied()).collect(),
        (ColorType::Grayscale, BitDepth::Two | BitDepth::Four) => {
            let scale = if depth == 2 { 0x55 } else { 0x11 };
            rows.flat_map(|r| unpack(r, depth, w).map(move |v| v * scale))
                .collect()
        }
        (ColorType::Indexed, BitDepth::One | BitDepth::Two | BitDepth::Four) => {
            rows.flat_map(|r| unpack(r, depth, w)).collect()
        }
        // `I;16`: native little-endian 16-bit gray.
        (ColorType::Grayscale, BitDepth::Sixteen) => rows
            .flat_map(|r| r[..2 * w].chunks_exact(2).flat_map(|s| [s[1], s[0]]))
            .collect(),
        (ColorType::GrayscaleAlpha, BitDepth::Sixteen) => rows
            .flat_map(|r| {
                r[..4 * w]
                    .chunks_exact(4)
                    .flat_map(|s| [s[0], s[0], s[0], s[2]])
            })
            .collect(),
        // 16-bit RGB/RGBA: keep the most significant byte of each sample.
        (ColorType::Rgb | ColorType::Rgba, BitDepth::Sixteen) => {
            let n = info.color_type.samples() * w * 2;
            rows.flat_map(|r| r[..n].iter().step_by(2).copied())
                .collect()
        }
        // 8-bit L / LA / RGB / RGBA / P: the samples are the mode's bytes.
        (_, BitDepth::Eight) => {
            let n = info.color_type.samples() * w;
            rows.flat_map(|r| r[..n].iter().copied()).collect()
        }
        _ => return None,
    };
    Some(out)
}

/// The first `w` sub-byte samples (`depth` 1, 2 or 4 bits, MSB-first) of a
/// packed scanline, one value per byte.
fn unpack(row: &[u8], depth: u8, w: usize) -> impl Iterator<Item = u8> + '_ {
    let per = 8 / depth as usize;
    let mask = (1u16 << depth) as u8 - 1;
    (0..w).map(move |i| {
        let shift = 8 - depth as usize * (i % per + 1);
        (row[i / per] >> shift) & mask
    })
}

/// The `(mimetype, data: URI)` docling writes for an inline image:
/// `ImageRef.from_pil` always stores a PNG, so a JPEG is re-encoded from its
/// libjpeg pixels (the same pixels docling's PIL decode yields); a PNG — or
/// an encoding we cannot decode — keeps its bytes and its own type.
pub(crate) fn docling_data_uri(img: &crate::PictureImage) -> (String, String) {
    match asset_png(&img.data) {
        Some(png) if !is_png(&img.data) => (
            "image/png".to_string(),
            format!("data:image/png;base64,{}", crate::base64::encode(&png)),
        ),
        _ => (img.mimetype.clone(), img.data_uri()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal PNG of `w`×`h` with the given IHDR depth/colour type, raw
    /// (filter-0) scanlines and an optional PLTE.
    fn png(w: u32, h: u32, depth: u8, color: u8, rows: &[&[u8]], plte: Option<&[u8]>) -> Vec<u8> {
        let mut enc_buf = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut enc_buf, w, h);
            enc.set_depth(png::BitDepth::from_u8(depth).unwrap());
            enc.set_color(png::ColorType::from_u8(color).unwrap());
            if let Some(p) = plte {
                enc.set_palette(p.to_vec());
            }
            let mut wr = enc.write_header().unwrap();
            let data: Vec<u8> = rows.iter().flat_map(|r| r.iter().copied()).collect();
            wr.write_image_data(&data).unwrap();
        }
        enc_buf
    }

    // Expected bytes cross-checked against Pillow 12.3: `Image.open(f).tobytes()`,
    // identical again after the backends' `save(format="PNG")` + reopen.

    #[test]
    fn rgb8_is_the_raw_samples() {
        let f = png(2, 1, 8, 2, &[&[1, 2, 3, 4, 5, 6]], None);
        assert_eq!(pil_png_bytes(&f).unwrap(), vec![1, 2, 3, 4, 5, 6]);
        assert_ne!(image_digest(&f), sha256_hex(&f));
    }

    #[test]
    fn sub_byte_gray_is_scaled_to_l() {
        let f = png(3, 1, 2, 0, &[&[0b00_01_10_11]], None);
        assert_eq!(pil_png_bytes(&f).unwrap(), vec![0x00, 0x55, 0xaa]);
        let f = png(3, 1, 4, 0, &[&[0x0f, 0x10]], None);
        assert_eq!(pil_png_bytes(&f).unwrap(), vec![0x00, 0xff, 0x11]);
    }

    #[test]
    fn one_bit_gray_stays_packed_and_palette_unpacks_to_indices() {
        let f = png(10, 1, 1, 0, &[&[0b1010_0000, 0b1100_0000]], None);
        assert_eq!(pil_png_bytes(&f).unwrap(), vec![0b1010_0000, 0b1100_0000]);
        let f = png(3, 1, 2, 3, &[&[0b11_01_00_00]], Some(&[0; 12]));
        assert_eq!(pil_png_bytes(&f).unwrap(), vec![3, 1, 0]);
    }

    #[test]
    fn sixteen_bit_modes_follow_pillow() {
        // I;16 little-endian.
        let f = png(1, 1, 16, 0, &[&[0x12, 0x34]], None);
        assert_eq!(pil_png_bytes(&f).unwrap(), vec![0x34, 0x12]);
        // RGB;16B → RGB (MSBs).
        let f = png(1, 1, 16, 2, &[&[1, 2, 3, 4, 5, 6]], None);
        assert_eq!(pil_png_bytes(&f).unwrap(), vec![1, 3, 5]);
        // LA;16B → RGBA.
        let f = png(1, 1, 16, 4, &[&[7, 8, 9, 10]], None);
        assert_eq!(pil_png_bytes(&f).unwrap(), vec![7, 7, 7, 9]);
    }

    /// docling-pdf's libjpeg fixtures (one crate over); digests from Pillow
    /// 12.3: `sha256(Image.open(f).tobytes())`, unchanged by the PNG round-trip.
    #[test]
    fn jpeg_digest_is_pillows_libjpeg_decode() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docling-pdf/tests/data/jpeg");
        for (name, want) in [
            (
                "rgb_420",
                "50690f45a9c80a48b5e2e0e38008cd3ac26b8d92af24f2acddbdabcb6ec0c4c1",
            ),
            (
                "gray_progressive",
                "dc231f8f1ef7b807f59587511af8a8d1b830b4e0cff8a185aa16188195641f06",
            ),
            (
                "rgb_444_progressive",
                "e78a1d2a7181f6781e71b695d918f431e8735ecea30fc14983206f5cbd214a1e",
            ),
        ] {
            let Ok(jpg) = std::fs::read(dir.join(format!("{name}.jpg"))) else {
                return; // a packaged docling-core has no sibling crate
            };
            assert_eq!(image_digest(&jpg), want, "{name}");
            // The archive part is a PNG of exactly those pixels.
            let png = asset_png(&jpg).unwrap();
            assert_eq!(
                pil_png_bytes(&png).map(|b| sha256_hex(&b)).as_deref(),
                Some(want)
            );
        }
    }

    /// The prepared cache (#898) answers exactly what the one-at-a-time
    /// functions do — PNG, JPEG, an encoding neither covers, duplicates —
    /// and an image it was not prepared with, or already handed out, too.
    #[test]
    fn prepared_assets_match_the_serial_functions() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docling-pdf/tests/data/jpeg");
        let jpg = std::fs::read(dir.join("rgb_420.jpg")).unwrap_or_default();
        let gray = png(2, 1, 8, 0, &[&[7, 200]], None);
        let rgb = png(1, 1, 8, 2, &[&[1, 2, 3]], None);
        let gif = b"GIF89a-not-really".to_vec();
        let mut images: Vec<&[u8]> = vec![&gray, &rgb, &gif, &gray];
        if !jpg.is_empty() {
            images.push(&jpg);
        }
        let mut cache = Assets::prepare(images.iter().copied());
        assert_eq!(
            cache.map.len(),
            images.len() - 1,
            "the duplicate is one entry"
        );
        for data in &images {
            assert_eq!(cache.digest(data), image_digest(data));
        }
        for data in &images {
            let want = asset_png(data).unwrap_or_else(|| data.to_vec());
            assert_eq!(cache.take_png(data), want, "first take");
            assert_eq!(cache.take_png(data), want, "re-encoded after the take");
        }
        // Not prepared: decoded on the spot.
        let other = png(1, 1, 8, 0, &[&[9]], None);
        assert_eq!(cache.digest(&other), image_digest(&other));
        assert_eq!(cache.take_png(&other), other);
        let empty = Assets::empty();
        assert_eq!(empty.digest(&rgb), image_digest(&rgb));
    }

    #[test]
    fn parallel_map_keeps_item_order() {
        let items: Vec<u32> = (0..1000).collect();
        assert_eq!(
            parallel_map(&items, |&x| x * 3),
            items.iter().map(|x| x * 3).collect::<Vec<_>>()
        );
        assert_eq!(parallel_map(&[5u32], |&x| x + 1), vec![6]);
        assert!(parallel_map::<u32, u32>(&[], |&x| x).is_empty());
    }

    #[test]
    fn non_png_keeps_the_encoded_bytes_digest() {
        assert_eq!(image_digest(b"\xff\xd8\xff"), sha256_hex(b"\xff\xd8\xff"));
    }

    #[test]
    fn matches_docling_for_the_issue_527_fixture_image() {
        // word/media/image1.png of rich_merged_span_list_image_nested.docx;
        // docling names it image_000000_fa7b78cc….png.
        const HEX: &str = "89504e470d0a1a0a0000000d49484452000000080000000808020000004b6d29dc\
                           0000000f49444154789c6368c001188696040082f360019cee0f2400000000\
                           49454e44ae426082";
        let f: Vec<u8> = (0..HEX.len() / 2)
            .map(|i| u8::from_str_radix(&HEX[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        assert_eq!(
            image_digest(&f),
            "fa7b78cc215df21d7ce54d8c3c6637c326dab95c10fbc12263101365973f4268"
        );
    }
}
