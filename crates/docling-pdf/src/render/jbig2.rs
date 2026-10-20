//! `JBIG2Decode` images (ISO 32000-1, 7.4.7): the embedded JBIG2 streams of
//! scanned PDFs, decoded with `hayro-jbig2` (#811).
//!
//! A scanned page's text is often a JBIG2 `/ImageMask` stencil drawn over
//! a JPEG background. Without a decoder the stencil drew nothing — not
//! even the mid-gray placeholder the other undecodable images get, since a
//! stencil paints only where it has samples — so such a page rendered as
//! its background alone, blank to the layout model and to OCR, with no
//! signal but a debug line. Decoded, the stencil is the text it carries.
//!
//! What the filter decides (7.4.7): the stream is the embedded organization
//! of T.88 Annex D.3 — a sequence of segments, no file header — and the
//! filter parameter `/JBIG2Globals` names a stream of segments (symbol
//! dictionaries, tables) shared by every image of the document, decoded
//! ahead of the image's own. The filter's output is 1 bit per pixel with
//! **0 = black**: JBIG2 codes black as 1, the PDF filter inverts it so that
//! the samples read as `DeviceGray` 1-bit (0 = black) and a stencil with the
//! default `/Decode [0 1]` paints its black pixels. The image dictionary's
//! `/Width` × `/Height` is what the samples are read as; a stream whose own
//! size differs is cropped or padded white, as the CCITT path does.

use hayro_jbig2::{Decoder, Image};

/// The decoded samples, packed 1 bit per pixel, `width.div_ceil(8)` bytes
/// per row for `height` rows, 0 = black (the filter's output, 7.4.7).
pub struct Decoded {
    pub stride: usize,
    pub data: Vec<u8>,
}

/// Collects the decoder's pixels into packed rows of the image's own width;
/// `next_line` files each row into the output at the dictionary's size.
struct Rows {
    /// The image's width (the decoder pushes exactly this many pixels per
    /// row) and the row under construction, 1 = white.
    image_width: usize,
    row: Vec<u8>,
    x: usize,
    /// The output at the dictionary's size.
    stride: usize,
    height: usize,
    out: Vec<u8>,
    y: usize,
}

impl Rows {
    fn file_row(&mut self) {
        if self.y < self.height {
            let n = self.stride.min(self.row.len());
            let at = self.y * self.stride;
            self.out[at..at + n].copy_from_slice(&self.row[..n]);
        }
        self.y += 1;
        self.row.fill(0xff);
        self.x = 0;
    }
}

impl Decoder for Rows {
    fn push_pixel(&mut self, black: bool) {
        if black && self.x < self.image_width {
            self.row[self.x / 8] &= !(0x80 >> (self.x % 8));
        }
        self.x += 1;
    }

    fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32) {
        // Byte-aligned by contract: whole bytes of one colour.
        let bytes = chunk_count as usize;
        if black {
            let start = (self.x / 8).min(self.row.len());
            let end = (start + bytes).min(self.row.len());
            self.row[start..end].fill(0x00);
        }
        self.x += bytes * 8;
    }

    fn next_line(&mut self) {
        self.file_row();
    }
}

/// Decode `data` (an embedded JBIG2 stream) with `globals` (the
/// `/JBIG2Globals` stream's content, already unfiltered) as the filter
/// would, into `width` × `height` packed 1-bit rows, 0 = black.
pub fn decode(
    data: &[u8],
    globals: Option<&[u8]>,
    width: usize,
    height: usize,
) -> Result<Decoded, String> {
    if width == 0 || height == 0 {
        return Err("empty image".into());
    }
    let image = Image::new_embedded(data, globals).map_err(|e| format!("{e:?}"))?;
    let (iw, ih) = (image.width() as usize, image.height() as usize);
    if iw == 0 || ih == 0 {
        return Err(format!("JBIG2 image is {iw} × {ih}"));
    }
    // The same ceiling as the renderer's other decoders: a crafted header
    // must not allocate gigabytes.
    if iw.saturating_mul(ih) > 80_000_000 || width.saturating_mul(height) > 80_000_000 {
        return Err(format!("JBIG2 image too large ({iw} × {ih})"));
    }
    let stride = width.div_ceil(8);
    let mut rows = Rows {
        image_width: iw,
        // One byte of slack: a chunk may run past the last partial byte.
        row: vec![0xff; iw.div_ceil(8) + 1],
        x: 0,
        stride,
        height,
        out: vec![0xff; stride * height],
        y: 0,
    };
    image.decode(&mut rows).map_err(|e| format!("{e:?}"))?;
    // A last row the decoder did not close.
    if rows.x > 0 {
        rows.file_row();
    }
    Ok(Decoded {
        stride,
        data: rows.out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/data/jbig2")
                .join(name),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    /// The reference bitmap of a fixture, 1 bit per pixel packed like the
    /// filter's output (0 = black), from the PNG the decoder's own test
    /// suite checks against.
    fn reference(name: &str) -> (usize, usize, Vec<u8>) {
        let img = image::load_from_memory(&fixture(name)).unwrap().to_luma8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let stride = w.div_ceil(8);
        let mut out = vec![0xffu8; stride * h];
        for (y, row) in img.rows().enumerate() {
            for (x, p) in row.enumerate() {
                if p.0[0] < 128 {
                    out[y * stride + x / 8] &= !(0x80 >> (x % 8));
                }
            }
        }
        (w, h, out)
    }

    /// An MMR-coded generic region (the T.6 coding a scanner's JBIG2 shares
    /// with CCITT G4) decodes to the reference, 0 = black.
    #[test]
    fn mmr_generic_region_matches_the_reference() {
        let (w, h, want) = reference("generic_mmr.png");
        let got = decode(&fixture("generic_mmr.jbig2"), None, w, h).unwrap();
        assert_eq!(got.stride, w.div_ceil(8));
        assert_eq!(got.data, want);
        // Some black: the reference is a picture, not a blank page.
        assert!(got.data.iter().any(|&b| b != 0xff));
    }

    /// A symbol dictionary in `/JBIG2Globals` and the text region that uses
    /// it in the image's own stream — the layout a PDF producer writes for
    /// a scanned document's text.
    #[test]
    fn text_region_with_global_symbol_dictionary() {
        let (w, h, want) = reference("symbol_global.png");
        let globals = fixture("symbol_global.globals.jbig2");
        let got = decode(&fixture("symbol_global.jbig2"), Some(&globals), w, h).unwrap();
        assert_eq!(got.data, want);
        // Without its globals the text region has no symbols to draw.
        assert!(decode(&fixture("symbol_global.jbig2"), None, w, h).is_err());
    }

    /// The dictionary's size wins: a wider or taller declaration pads white,
    /// a smaller one crops.
    #[test]
    fn dictionary_size_crops_or_pads() {
        let (w, h, want) = reference("generic_mmr.png");
        let padded = decode(&fixture("generic_mmr.jbig2"), None, w + 9, h + 2).unwrap();
        assert_eq!(padded.stride, (w + 9).div_ceil(8));
        for y in 0..h {
            let got = &padded.data[y * padded.stride..y * padded.stride + w.div_ceil(8)];
            let exp = &want[y * w.div_ceil(8)..(y + 1) * w.div_ceil(8)];
            // The bits past the image width are white in both.
            let mask = 0xffu8 << ((8 - w % 8) % 8);
            let (gl, el) = (got.last().unwrap() & mask, exp.last().unwrap() & mask);
            assert_eq!(&got[..got.len() - 1], &exp[..exp.len() - 1], "row {y}");
            assert_eq!(gl, el, "row {y} tail");
        }
        assert!(padded.data[h * padded.stride..].iter().all(|&b| b == 0xff));
        let cropped = decode(&fixture("generic_mmr.jbig2"), None, 8, 3).unwrap();
        assert_eq!(cropped.data.len(), 3);
        assert_eq!(
            &cropped.data[..],
            &[want[0], want[w.div_ceil(8)], want[2 * w.div_ceil(8)]]
        );
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode(b"not a jbig2 stream", None, 10, 10).is_err());
        assert!(decode(&[], None, 10, 10).is_err());
        assert!(decode(&fixture("generic_mmr.jbig2"), None, 0, 10).is_err());
    }
}
