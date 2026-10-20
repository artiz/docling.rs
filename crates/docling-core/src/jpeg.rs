//! A JPEG decoder that reproduces libjpeg(-turbo)'s output byte for byte —
//! what pdfium's `DCTDecode` produces (`core/fxcodec/jpeg`, libjpeg at its
//! defaults: the `JDCT_ISLOW` integer IDCT, "fancy" triangle-filter chroma
//! upsampling, the 16.16 fixed-point YCbCr→RGB tables of `jdcolor.c`).
//!
//! Neither pure-Rust decoder in the dependency tree matches those bytes:
//! `zune-jpeg` (the `image` crate's) rounds its upsampler and colour
//! conversion differently, `jpeg-decoder` carries stb_image's `+8/+8` h2v2
//! filter where libjpeg alternates `+8/+7` and a float-derived colour
//! conversion. The pixel differences are ±1, invisible — and exactly what a
//! byte-for-byte oracle against pdfium's raster cannot tolerate. So this is a
//! deliberately small decoder: baseline and progressive Huffman, 8-bit,
//! 1 or 3 components, restart intervals, and libjpeg's `1/2`, `1/4`, `1/8`
//! DCT-scaled output (`jidctred.c`'s 4×4/2×2/1×1 transforms with
//! `jdmaster.c`'s per-component scaling and the scaled upsampler rules);
//! everything else (arithmetic coding, 12-bit, lossless, CMYK/YCCK, DNL) is
//! reported as unsupported and the page stays with pdfium. Checked against
//! Pillow's libjpeg-turbo on docling-pdf's `tests/data/jpeg/` fixtures
//! (`raster::jpeg_tests::matches_libjpeg_on_the_fixtures`).
//!
//! It lives in docling-core because Pillow decodes with the same libjpeg:
//! besides docling-pdf's raster/renderer (which re-export it), the DocLang
//! serializer hashes a JPEG picture's decoded pixels to name its asset the
//! way docling does (`pixel_digest.rs`).

/// A decoded image: `channels` is 1 (gray) or 3 (RGB, or the raw component
/// triplets when no colour transform applies), rows of `width` pixels.
#[derive(Debug, Clone)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    pub data: Vec<u8>,
    /// A four-component image carried an Adobe APP14 marker: its CMYK
    /// samples are stored inverted (Adobe's convention), as libjpeg hands
    /// them out.
    pub adobe_inverted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Unsupported(&'static str),
    Corrupt(&'static str),
}

/// Header facts the caller needs before decoding.
#[derive(Debug, Clone, Copy)]
pub struct Info {
    pub width: usize,
    pub height: usize,
    pub components: usize,
}

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

#[derive(Clone, Default)]
struct Huffman {
    /// `maxcode[l]` for code length `l` (1..=16), -1 where no code has that
    /// length; `valptr[l]` the index of the first value of that length and
    /// `mincode[l]` its code — libjpeg's slow-path decode (`jpeg_huff_decode`).
    maxcode: [i32; 18],
    valptr: [i32; 17],
    mincode: [i32; 17],
    values: Vec<u8>,
    present: bool,
}

impl Huffman {
    fn build(bits: &[u8; 16], values: Vec<u8>) -> Result<Self, Error> {
        let mut h = Huffman {
            values,
            present: true,
            ..Default::default()
        };
        let mut code: i32 = 0;
        let mut k: i32 = 0;
        for l in 1..=16usize {
            let n = i32::from(bits[l - 1]);
            if n == 0 {
                h.maxcode[l] = -1;
            } else {
                h.valptr[l] = k;
                h.mincode[l] = code;
                code += n;
                k += n;
                h.maxcode[l] = code - 1;
            }
            code <<= 1;
        }
        h.maxcode[17] = i32::MAX;
        if k as usize != h.values.len() {
            return Err(Error::Corrupt("DHT value count"));
        }
        Ok(h)
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    nbits: u32,
    /// A marker met inside the entropy-coded data: bits after it read as
    /// zeros, as libjpeg's `jpeg_fill_bit_buffer` fills them.
    marker: Option<u8>,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Reader {
            data,
            pos,
            acc: 0,
            nbits: 0,
            marker: None,
        }
    }

    fn fill(&mut self) {
        while self.nbits <= 56 {
            let byte = if self.marker.is_some() {
                0
            } else {
                match self.data.get(self.pos) {
                    None => {
                        self.marker = Some(0xD9);
                        0
                    }
                    Some(&0xFF) => {
                        let next = self.data.get(self.pos + 1).copied().unwrap_or(0xD9);
                        if next == 0 {
                            self.pos += 2;
                            0xFF
                        } else if next == 0xFF {
                            // Fill byte before a marker.
                            self.pos += 1;
                            continue;
                        } else {
                            self.marker = Some(next);
                            0
                        }
                    }
                    Some(&b) => {
                        self.pos += 1;
                        b
                    }
                }
            };
            self.acc |= u64::from(byte) << (56 - self.nbits);
            self.nbits += 8;
        }
    }

    fn bits(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if self.nbits < n {
            self.fill();
        }
        let v = (self.acc >> (64 - n)) as u32;
        self.acc <<= n;
        self.nbits -= n;
        v
    }

    fn bit(&mut self) -> u32 {
        self.bits(1)
    }

    /// `HUFF_EXTEND`: the `s`-bit magnitude category to a signed value.
    fn receive_extend(&mut self, s: u32) -> i32 {
        if s == 0 {
            return 0;
        }
        let v = self.bits(s) as i32;
        if v < (1 << (s - 1)) {
            v - (1 << s) + 1
        } else {
            v
        }
    }

    fn decode(&mut self, h: &Huffman) -> Result<u8, Error> {
        if !h.present {
            return Err(Error::Corrupt("missing Huffman table"));
        }
        let mut code = self.bit() as i32;
        let mut l = 1usize;
        while code > h.maxcode[l] {
            code = (code << 1) | self.bit() as i32;
            l += 1;
            if l > 16 {
                // libjpeg warns and yields 0 here.
                return Ok(0);
            }
        }
        let idx = h.valptr[l] + code - h.mincode[l];
        Ok(h.values.get(idx as usize).copied().unwrap_or(0))
    }

    /// Drop the buffered bits and the marker note (restart / end of scan).
    fn reset(&mut self) {
        self.acc = 0;
        self.nbits = 0;
        self.marker = None;
    }

    /// Position of the next marker's `0xFF` after the entropy-coded segment.
    fn marker_position(&self) -> usize {
        // Bytes still in the accumulator were consumed from `pos` already,
        // so the marker (if seen) sits exactly at `pos`.
        if self.marker.is_some() {
            return self.pos;
        }
        let mut p = self.pos;
        while p + 1 < self.data.len() {
            if self.data[p] == 0xFF && self.data[p + 1] != 0 && self.data[p + 1] != 0xFF {
                return p;
            }
            p += 1;
        }
        self.data.len()
    }
}

struct Component {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    /// Blocks per line / rows of the padded (whole-MCU) grid.
    bw: usize,
    bh: usize,
    /// `downsampled_width/height` after IDCT scaling:
    /// `ceil(X·h·dct/(hmax·8))`, `ceil(Y·v·dct/(vmax·8))`.
    dw: usize,
    dh: usize,
    /// `DCT_scaled_size`: the IDCT output size per block (8, 4, 2 or 1) —
    /// libjpeg scales chroma up through the IDCT rather than the upsampler
    /// where the sampling ratios allow (`jpeg_calc_output_dimensions`).
    dct: usize,
    /// Coefficients in natural order, `bw·bh` blocks of 64 (progressive
    /// accumulates here; baseline reconstructs each block on the spot).
    coefs: Vec<i16>,
    /// Reconstructed samples, `bw·dct` × `bh·dct`.
    samples: Vec<u8>,
    dc_tbl: usize,
    ac_tbl: usize,
    dc_pred: i32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ColorSpace {
    Gray,
    YCbCr,
    Rgb,
}

struct Decoder<'a> {
    data: &'a [u8],
    qt: [[u16; 64]; 4],
    qt_present: [bool; 4],
    dc: [Huffman; 4],
    ac: [Huffman; 4],
    comps: Vec<Component>,
    width: usize,
    height: usize,
    hmax: usize,
    vmax: usize,
    mcux: usize,
    mcuy: usize,
    progressive: bool,
    restart_interval: usize,
    saw_jfif: bool,
    adobe_transform: Option<u8>,
    eobrun: u32,
    /// `min_DCT_scaled_size` for the requested `1/scale_denom`: 8, 4, 2 or 1.
    min_dct: usize,
}

/// Read the frame header only.
pub fn info(data: &[u8]) -> Result<Info, Error> {
    let mut d = Decoder::new(data, 1)?;
    d.run(true)?;
    Ok(Info {
        width: d.width,
        height: d.height,
        components: d.comps.len(),
    })
}

/// Decode `data` at `1/scale_denom` (1, 2, 4 or 8 — libjpeg's DCT scaling,
/// which pdfium requests for an image at least twice the bitmap's size:
/// `resolution_levels_to_skip`); the output is `ceil(w/scale_denom)` ×
/// `ceil(h/scale_denom)`. `color_transform` is the PDF's `/ColorTransform`
/// decode parameter (default 1), which pdfium forces on when an Adobe marker
/// is present and otherwise uses to decide whether a 3-component image is
/// converted from YCbCr or handed over raw.
pub fn decode(data: &[u8], color_transform: bool, scale_denom: u32) -> Result<Image, Error> {
    let mut d = Decoder::new(data, scale_denom)?;
    d.run(false)?;
    d.finish(color_transform)
}

impl<'a> Decoder<'a> {
    fn new(data: &'a [u8], scale_denom: u32) -> Result<Self, Error> {
        // `jpeg_core_output_dimensions` for `scale_num = 1`.
        let min_dct = match scale_denom {
            1 => 8,
            2 => 4,
            4 => 2,
            8 => 1,
            _ => return Err(Error::Unsupported("DCT scale")),
        };
        Ok(Decoder {
            data,
            qt: [[0; 64]; 4],
            qt_present: [false; 4],
            dc: Default::default(),
            ac: Default::default(),
            comps: Vec::new(),
            width: 0,
            height: 0,
            hmax: 1,
            vmax: 1,
            mcux: 0,
            mcuy: 0,
            progressive: false,
            restart_interval: 0,
            saw_jfif: false,
            adobe_transform: None,
            eobrun: 0,
            min_dct,
        })
    }

    fn u16_at(&self, p: usize) -> Result<usize, Error> {
        match (self.data.get(p), self.data.get(p + 1)) {
            (Some(&a), Some(&b)) => Ok(usize::from(a) << 8 | usize::from(b)),
            _ => Err(Error::Corrupt("truncated")),
        }
    }

    /// Walk the marker segments; with `header_only`, stop at the first SOS.
    fn run(&mut self, header_only: bool) -> Result<(), Error> {
        let mut p = 0usize;
        // Tolerate leading garbage before SOI, as libjpeg does not but pdfium's
        // stream boundaries do.
        while p + 1 < self.data.len() && !(self.data[p] == 0xFF && self.data[p + 1] == 0xD8) {
            p += 1;
        }
        if p + 1 >= self.data.len() {
            return Err(Error::Corrupt("no SOI"));
        }
        p += 2;
        loop {
            // Skip fill bytes to the next marker.
            while p < self.data.len() && self.data[p] != 0xFF {
                p += 1;
            }
            while p < self.data.len() && self.data[p] == 0xFF {
                p += 1;
            }
            let Some(&marker) = self.data.get(p) else {
                return if self.comps.is_empty() {
                    Err(Error::Corrupt("no frame"))
                } else {
                    Ok(())
                };
            };
            p += 1;
            match marker {
                0xD8 | 0x01 | 0xD0..=0xD7 => continue, // standalone markers
                0xD9 => return Ok(()),                 // EOI
                _ => {}
            }
            let len = self.u16_at(p)?;
            if len < 2 {
                return Err(Error::Corrupt("segment length"));
            }
            let seg = self
                .data
                .get(p + 2..p + len)
                .ok_or(Error::Corrupt("truncated segment"))?;
            match marker {
                0xC0..=0xC2 => {
                    self.progressive = marker == 0xC2;
                    self.frame(seg)?;
                }
                0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                    return Err(Error::Unsupported(
                        "lossless, hierarchical or arithmetic JPEG",
                    ));
                }
                0xC4 => self.dht(seg)?,
                0xDB => self.dqt(seg)?,
                0xDD => {
                    if seg.len() < 2 {
                        return Err(Error::Corrupt("DRI"));
                    }
                    self.restart_interval = usize::from(seg[0]) << 8 | usize::from(seg[1]);
                }
                0xDC => return Err(Error::Unsupported("DNL")),
                0xE0 => {
                    if seg.starts_with(b"JFIF\0") {
                        self.saw_jfif = true;
                    }
                }
                0xEE => {
                    if seg.starts_with(b"Adobe") && seg.len() >= 12 {
                        self.adobe_transform = Some(seg[11]);
                    }
                }
                0xDA => {
                    if header_only {
                        return if self.comps.is_empty() {
                            Err(Error::Corrupt("SOS before SOF"))
                        } else {
                            Ok(())
                        };
                    }
                    let end = self.scan(seg, p + len)?;
                    p = end;
                    continue;
                }
                _ => {}
            }
            p += len;
        }
    }

    fn frame(&mut self, seg: &[u8]) -> Result<(), Error> {
        if seg.len() < 6 {
            return Err(Error::Corrupt("SOF"));
        }
        if seg[0] != 8 {
            return Err(Error::Unsupported("sample precision other than 8"));
        }
        self.height = usize::from(seg[1]) << 8 | usize::from(seg[2]);
        self.width = usize::from(seg[3]) << 8 | usize::from(seg[4]);
        let n = usize::from(seg[5]);
        if self.height == 0 {
            return Err(Error::Unsupported("DNL-defined height"));
        }
        if self.width == 0 || !(n == 1 || n == 3 || n == 4) {
            return Err(Error::Unsupported("component count"));
        }
        if seg.len() < 6 + 3 * n {
            return Err(Error::Corrupt("SOF components"));
        }
        self.comps.clear();
        for i in 0..n {
            let c = &seg[6 + 3 * i..9 + 3 * i];
            let (h, v) = (usize::from(c[1] >> 4), usize::from(c[1] & 15));
            if !(1..=4).contains(&h) || !(1..=4).contains(&v) || c[2] > 3 {
                return Err(Error::Corrupt("sampling factors"));
            }
            self.comps.push(Component {
                id: c[0],
                h,
                v,
                tq: usize::from(c[2]),
                bw: 0,
                bh: 0,
                dw: 0,
                dh: 0,
                dct: 8,
                coefs: Vec::new(),
                samples: Vec::new(),
                dc_tbl: 0,
                ac_tbl: 0,
                dc_pred: 0,
            });
        }
        self.hmax = self.comps.iter().map(|c| c.h).max().unwrap_or(1);
        self.vmax = self.comps.iter().map(|c| c.v).max().unwrap_or(1);
        self.mcux = self.width.div_ceil(8 * self.hmax);
        self.mcuy = self.height.div_ceil(8 * self.vmax);
        let (w, h, hmax, vmax, mcux, mcuy, progressive, min_dct) = (
            self.width,
            self.height,
            self.hmax,
            self.vmax,
            self.mcux,
            self.mcuy,
            self.progressive,
            self.min_dct,
        );
        for c in &mut self.comps {
            c.bw = mcux * c.h;
            c.bh = mcuy * c.v;
            // `jpeg_calc_output_dimensions`: scale a subsampled component up
            // through the IDCT while the ratios stay integral. (`%`, not
            // `is_multiple_of`: docling-core's MSRV is 1.85; the sampling
            // factors are validated 1..=4, so no divisor is zero.)
            let mut ssize = min_dct;
            while ssize < 8
                && (hmax * min_dct) % (c.h * ssize * 2) == 0
                && (vmax * min_dct) % (c.v * ssize * 2) == 0
            {
                ssize *= 2;
            }
            c.dct = ssize;
            c.dw = (w * c.h * c.dct).div_ceil(hmax * 8);
            c.dh = (h * c.v * c.dct).div_ceil(vmax * 8);
            let blocks = c.bw * c.bh;
            if blocks > (1usize << 26) {
                return Err(Error::Unsupported("image too large"));
            }
            if progressive {
                c.coefs = vec![0; blocks * 64];
            }
            c.samples = vec![0; blocks * c.dct * c.dct];
        }
        Ok(())
    }

    fn dht(&mut self, mut seg: &[u8]) -> Result<(), Error> {
        while !seg.is_empty() {
            if seg.len() < 17 {
                return Err(Error::Corrupt("DHT"));
            }
            let class = seg[0] >> 4;
            let id = usize::from(seg[0] & 15);
            if id > 3 || class > 1 {
                return Err(Error::Corrupt("DHT id"));
            }
            let mut bits = [0u8; 16];
            bits.copy_from_slice(&seg[1..17]);
            let total: usize = bits.iter().map(|&b| usize::from(b)).sum();
            if total > 256 || seg.len() < 17 + total {
                return Err(Error::Corrupt("DHT counts"));
            }
            let table = Huffman::build(&bits, seg[17..17 + total].to_vec())?;
            if class == 0 {
                self.dc[id] = table;
            } else {
                self.ac[id] = table;
            }
            seg = &seg[17 + total..];
        }
        Ok(())
    }

    fn dqt(&mut self, mut seg: &[u8]) -> Result<(), Error> {
        while !seg.is_empty() {
            let pq = seg[0] >> 4;
            let tq = usize::from(seg[0] & 15);
            if tq > 3 || pq > 1 {
                return Err(Error::Corrupt("DQT id"));
            }
            let n = if pq == 0 { 64 } else { 128 };
            if seg.len() < 1 + n {
                return Err(Error::Corrupt("DQT"));
            }
            for k in 0..64 {
                let v = if pq == 0 {
                    u16::from(seg[1 + k])
                } else {
                    u16::from(seg[1 + 2 * k]) << 8 | u16::from(seg[2 + 2 * k])
                };
                self.qt[tq][ZIGZAG[k]] = v;
            }
            self.qt_present[tq] = true;
            seg = &seg[1 + n..];
        }
        Ok(())
    }

    /// Decode one scan whose entropy-coded data starts at `start`; returns
    /// the position of the marker that ends it.
    fn scan(&mut self, seg: &[u8], start: usize) -> Result<usize, Error> {
        if self.comps.is_empty() {
            return Err(Error::Corrupt("SOS before SOF"));
        }
        let ns = usize::from(*seg.first().ok_or(Error::Corrupt("SOS"))?);
        if ns == 0 || ns > 4 || seg.len() < 1 + 2 * ns + 3 {
            return Err(Error::Corrupt("SOS"));
        }
        let mut in_scan = Vec::with_capacity(ns);
        for i in 0..ns {
            let cs = seg[1 + 2 * i];
            let t = seg[2 + 2 * i];
            let ci = self
                .comps
                .iter()
                .position(|c| c.id == cs)
                .ok_or(Error::Corrupt("SOS component"))?;
            self.comps[ci].dc_tbl = usize::from(t >> 4).min(3);
            self.comps[ci].ac_tbl = usize::from(t & 15).min(3);
            in_scan.push(ci);
        }
        let ss = usize::from(seg[1 + 2 * ns]);
        let se = usize::from(seg[2 + 2 * ns]);
        let ah = u32::from(seg[3 + 2 * ns] >> 4);
        let al = u32::from(seg[3 + 2 * ns] & 15);
        if self.progressive {
            if ss > se || se > 63 || (ss == 0 && se != 0) || (ss > 0 && ns != 1) || al > 13 {
                return Err(Error::Corrupt("progressive scan parameters"));
            }
        } else if ss != 0 || se != 63 || ah != 0 || al != 0 {
            return Err(Error::Corrupt("sequential scan parameters"));
        }
        for c in &mut self.comps {
            c.dc_pred = 0;
        }
        self.eobrun = 0;
        let mut rd = Reader::new(self.data, start);

        // MCU geometry: interleaved scans walk whole MCUs, a single-component
        // scan walks that component's own block grid (A.2.2) —
        // `width_in_blocks = ceil(image_width · h / (hmax · 8))`, the
        // *coded* size, whatever DCT scaling the output is asked for (the
        // scaled `dw`/`dh` would walk a fraction of the blocks and leave the
        // rest of a reduced grayscale or progressive decode black).
        let (mcus_x, mcus_y) = if ns == 1 {
            let c = &self.comps[in_scan[0]];
            (
                (self.width * c.h).div_ceil(self.hmax * 8),
                (self.height * c.v).div_ceil(self.vmax * 8),
            )
        } else {
            (self.mcux, self.mcuy)
        };
        let total = mcus_x * mcus_y;
        let mut count = 0usize;
        for my in 0..mcus_y {
            for mx in 0..mcus_x {
                if self.restart_interval > 0 && count > 0 && count % self.restart_interval == 0 {
                    self.restart(&mut rd);
                }
                if ns == 1 {
                    let ci = in_scan[0];
                    self.block(&mut rd, ci, mx, my, ss, se, ah, al)?;
                } else {
                    for &ci in &in_scan {
                        let (h, v) = (self.comps[ci].h, self.comps[ci].v);
                        for by in 0..v {
                            for bx in 0..h {
                                self.block(&mut rd, ci, mx * h + bx, my * v + by, ss, se, ah, al)?;
                            }
                        }
                    }
                }
                count += 1;
                if count == total {
                    break;
                }
            }
        }
        Ok(rd.marker_position())
    }

    /// Consume the RSTn marker between restart intervals and reset the
    /// predictors, as `read_restart_marker` does.
    fn restart(&mut self, rd: &mut Reader<'_>) {
        let mut p = rd.marker_position();
        // p points at 0xFF of the marker; skip 0xFF fill and the marker byte.
        while p < self.data.len() && self.data[p] == 0xFF {
            p += 1;
        }
        if p < self.data.len() && (0xD0..=0xD7).contains(&self.data[p]) {
            p += 1;
        }
        rd.reset();
        rd.pos = p;
        for c in &mut self.comps {
            c.dc_pred = 0;
        }
        self.eobrun = 0;
    }

    #[allow(clippy::too_many_arguments)]
    fn block(
        &mut self,
        rd: &mut Reader<'_>,
        ci: usize,
        bx: usize,
        by: usize,
        ss: usize,
        se: usize,
        ah: u32,
        al: u32,
    ) -> Result<(), Error> {
        let (bw, bh) = (self.comps[ci].bw, self.comps[ci].bh);
        if bx >= bw || by >= bh {
            return Err(Error::Corrupt("block outside the grid"));
        }
        let bi = by * bw + bx;
        if !self.progressive {
            let mut coef = [0i16; 64];
            let dc = &self.dc[self.comps[ci].dc_tbl];
            let ac = &self.ac[self.comps[ci].ac_tbl];
            let t = rd.decode(dc)?;
            let diff = rd.receive_extend(u32::from(t));
            let c = &mut self.comps[ci];
            c.dc_pred = c.dc_pred.wrapping_add(diff);
            coef[0] = c.dc_pred as i16;
            let mut k = 1usize;
            while k < 64 {
                let rs = rd.decode(ac)?;
                let r = usize::from(rs >> 4);
                let s = u32::from(rs & 15);
                if s == 0 {
                    if r == 15 {
                        k += 16;
                        continue;
                    }
                    break;
                }
                k += r;
                if k > 63 {
                    break;
                }
                coef[ZIGZAG[k]] = rd.receive_extend(s) as i16;
                k += 1;
            }
            let q = &self.qt[self.comps[ci].tq.min(3)];
            let c = &mut self.comps[ci];
            idct(c.dct, &coef, q, &mut c.samples, bi, bw);
            return Ok(());
        }

        // Progressive.
        let dc = &self.dc[self.comps[ci].dc_tbl];
        let ac = &self.ac[self.comps[ci].ac_tbl];
        let c = &mut self.comps[ci];
        let coef = &mut c.coefs[bi * 64..bi * 64 + 64];
        if ss == 0 {
            if ah == 0 {
                // DC first.
                let t = rd.decode(dc)?;
                let diff = rd.receive_extend(u32::from(t));
                c.dc_pred = c.dc_pred.wrapping_add(diff);
                coef[0] = (c.dc_pred << al) as i16;
            } else if rd.bit() == 1 {
                // DC refine.
                coef[0] |= (1i32 << al) as i16;
            }
            return Ok(());
        }
        if ah == 0 {
            // AC first (jdphuff.c decode_mcu_AC_first).
            if self.eobrun > 0 {
                self.eobrun -= 1;
                return Ok(());
            }
            let mut k = ss;
            while k <= se {
                let rs = rd.decode(ac)?;
                let r = u32::from(rs >> 4);
                let s = u32::from(rs & 15);
                if s != 0 {
                    k += r as usize;
                    if k > 63 {
                        break;
                    }
                    let v = rd.receive_extend(s);
                    coef[ZIGZAG[k]] = (v << al) as i16;
                } else {
                    if r != 15 {
                        self.eobrun = 1 << r;
                        if r > 0 {
                            self.eobrun += rd.bits(r);
                        }
                        self.eobrun -= 1;
                        break;
                    }
                    k += 15;
                }
                k += 1;
            }
            return Ok(());
        }
        // AC refine (jdphuff.c decode_mcu_AC_refine).
        let p1: i16 = (1i32 << al) as i16;
        let m1: i16 = (-1i32 << al) as i16;
        let mut k = ss;
        if self.eobrun == 0 {
            while k <= se {
                let rs = rd.decode(ac)?;
                let mut r = i32::from(rs >> 4);
                let mut s = i32::from(rs & 15);
                if s != 0 {
                    // Newly nonzero coefficient: sign bit.
                    s = if rd.bit() == 1 {
                        i32::from(p1)
                    } else {
                        i32::from(m1)
                    };
                } else if r != 15 {
                    self.eobrun = 1 << r;
                    if r > 0 {
                        self.eobrun += rd.bits(r as u32);
                    }
                    break;
                }
                // Advance over already-nonzero coefficients and `r` zero ones,
                // appending correction bits to the nonzero ones.
                loop {
                    let pos = ZIGZAG[k];
                    if coef[pos] != 0 {
                        if rd.bit() == 1 && (coef[pos] & p1) == 0 {
                            coef[pos] = if coef[pos] >= 0 {
                                coef[pos].wrapping_add(p1)
                            } else {
                                coef[pos].wrapping_add(m1)
                            };
                        }
                    } else {
                        r -= 1;
                        if r < 0 {
                            break;
                        }
                    }
                    k += 1;
                    if k > se {
                        break;
                    }
                }
                if s != 0 && k <= se {
                    coef[ZIGZAG[k]] = s as i16;
                }
                k += 1;
            }
        }
        if self.eobrun > 0 {
            while k <= se {
                let pos = ZIGZAG[k];
                if coef[pos] != 0 && rd.bit() == 1 && (coef[pos] & p1) == 0 {
                    coef[pos] = if coef[pos] >= 0 {
                        coef[pos].wrapping_add(p1)
                    } else {
                        coef[pos].wrapping_add(m1)
                    };
                }
                k += 1;
            }
            self.eobrun -= 1;
        }
        Ok(())
    }

    /// Reconstruct the progressive planes, upsample, convert.
    fn finish(mut self, color_transform: bool) -> Result<Image, Error> {
        if self.comps.is_empty() {
            return Err(Error::Corrupt("no frame"));
        }
        for c in &self.comps {
            if !self.qt_present[c.tq.min(3)] {
                return Err(Error::Corrupt("missing quantization table"));
            }
        }
        if self.progressive {
            for ci in 0..self.comps.len() {
                let q = self.qt[self.comps[ci].tq.min(3)];
                let c = &mut self.comps[ci];
                let bw = c.bw;
                for bi in 0..c.bw * c.bh {
                    let mut coef = [0i16; 64];
                    coef.copy_from_slice(&c.coefs[bi * 64..bi * 64 + 64]);
                    idct(c.dct, &coef, &q, &mut c.samples, bi, bw);
                }
                c.coefs = Vec::new();
            }
        }
        // `output_width/height`: `ceil(dim · min_DCT_scaled_size / 8)`.
        let (w, h) = (
            (self.width * self.min_dct).div_ceil(8),
            (self.height * self.min_dct).div_ceil(8),
        );
        let planes: Vec<Vec<u8>> = self.comps.iter().map(|c| self.upsample(c, w, h)).collect();
        let n = self.comps.len();
        let space = self.color_space();
        // pdfium: the /ColorTransform parameter, forced on by an Adobe marker;
        // off, a 3-component image is handed over in its own space.
        let convert = n == 3
            && space == ColorSpace::YCbCr
            && (color_transform || self.adobe_transform.is_some());
        let mut out = vec![0u8; w * h * n];
        if n == 1 {
            out.copy_from_slice(&planes[0][..w * h]);
        } else if n == 4 {
            // `ycck_cmyk_convert` (jdcolor.c) for Adobe transform 2, else
            // the four planes as stored (transform 0 = CMYK).
            let ycck = self.adobe_transform == Some(2);
            let t = ycc_tables();
            for i in 0..w * h {
                if ycck {
                    let y = i32::from(planes[0][i]);
                    let cb = usize::from(planes[1][i]);
                    let cr = usize::from(planes[2][i]);
                    out[4 * i] = range_limit(255 - (y + t.cr_r[cr]));
                    out[4 * i + 1] = range_limit(255 - (y + ((t.cb_g[cb] + t.cr_g[cr]) >> 16)));
                    out[4 * i + 2] = range_limit(255 - (y + t.cb_b[cb]));
                } else {
                    out[4 * i] = planes[0][i];
                    out[4 * i + 1] = planes[1][i];
                    out[4 * i + 2] = planes[2][i];
                }
                out[4 * i + 3] = planes[3][i];
            }
        } else if convert {
            let t = ycc_tables();
            for i in 0..w * h {
                let y = i32::from(planes[0][i]);
                let cb = usize::from(planes[1][i]);
                let cr = usize::from(planes[2][i]);
                out[3 * i] = range_limit(y + t.cr_r[cr]);
                out[3 * i + 1] = range_limit(y + ((t.cb_g[cb] + t.cr_g[cr]) >> 16));
                out[3 * i + 2] = range_limit(y + t.cb_b[cb]);
            }
        } else {
            for i in 0..w * h {
                out[3 * i] = planes[0][i];
                out[3 * i + 1] = planes[1][i];
                out[3 * i + 2] = planes[2][i];
            }
        }
        Ok(Image {
            width: w,
            height: h,
            channels: n,
            data: out,
            adobe_inverted: n == 4 && self.adobe_transform.is_some(),
        })
    }

    /// `default_decompress_parms`: what the components are.
    fn color_space(&self) -> ColorSpace {
        if self.comps.len() == 1 {
            return ColorSpace::Gray;
        }
        if self.saw_jfif {
            return ColorSpace::YCbCr;
        }
        if let Some(t) = self.adobe_transform {
            return if t == 0 {
                ColorSpace::Rgb
            } else {
                ColorSpace::YCbCr
            };
        }
        let ids = [self.comps[0].id, self.comps[1].id, self.comps[2].id];
        if ids == *b"RGB" {
            ColorSpace::Rgb
        } else {
            ColorSpace::YCbCr
        }
    }

    /// One component to the output size (`jinit_upsampler`): input groups
    /// of `h·dct/min_dct` × `v·dct/min_dct` samples become `hmax` × `vmax`
    /// — the triangle filters for 2:1 (only while `do_fancy`, i.e. the IDCT
    /// still produces more than one sample per block, and the row is wider
    /// than 2), replication otherwise, with libjpeg's edge rules (the row
    /// above the first / below the last is the edge row itself; the last
    /// column repeats).
    fn upsample(&self, c: &Component, w: usize, h: usize) -> Vec<u8> {
        let stride = c.bw * c.dct;
        let (dw, dh) = (c.dw, c.dh);
        let row = |r: usize| -> &[u8] {
            let r = r.min(dh.saturating_sub(1));
            &c.samples[r * stride..r * stride + dw]
        };
        let h_in = c.h * c.dct / self.min_dct;
        let v_in = c.v * c.dct / self.min_dct;
        let (h_out, v_out) = (self.hmax, self.vmax);
        let do_fancy = self.min_dct > 1;
        let mut out = vec![0u8; w * h];
        let replicate = |out: &mut Vec<u8>, h_exp: usize, v_exp: usize| {
            for y in 0..h {
                let src = row(y / v_exp);
                for x in 0..w {
                    out[y * w + x] = src[(x / h_exp).min(dw - 1)];
                }
            }
        };
        if h_in == h_out && v_in == v_out {
            for y in 0..h {
                out[y * w..y * w + w].copy_from_slice(&row(y)[..w]);
            }
        } else if h_in * 2 == h_out && v_in == v_out {
            if do_fancy && dw > 2 {
                let mut line = vec![0u8; dw * 2];
                for y in 0..h {
                    h2v1_fancy(row(y), &mut line);
                    out[y * w..y * w + w].copy_from_slice(&line[..w]);
                }
            } else {
                replicate(&mut out, 2, 1);
            }
        } else if h_in == h_out && v_in * 2 == v_out && do_fancy {
            for r in 0..dh {
                for v in 0..2 {
                    let y = 2 * r + v;
                    if y >= h {
                        break;
                    }
                    let near = row(r);
                    let far = if v == 0 {
                        row(r.saturating_sub(1))
                    } else {
                        row(r + 1)
                    };
                    let bias: u32 = if v == 0 { 1 } else { 2 };
                    for x in 0..w {
                        out[y * w + x] =
                            ((3 * u32::from(near[x]) + u32::from(far[x]) + bias) >> 2) as u8;
                    }
                }
            }
        } else if h_in * 2 == h_out && v_in * 2 == v_out {
            if do_fancy && dw > 2 {
                let mut line = vec![0u8; dw * 2];
                for r in 0..dh {
                    for v in 0..2 {
                        let y = 2 * r + v;
                        if y >= h {
                            break;
                        }
                        let near = row(r);
                        let far = if v == 0 {
                            row(r.saturating_sub(1))
                        } else {
                            row(r + 1)
                        };
                        h2v2_fancy(near, far, &mut line);
                        out[y * w..y * w + w].copy_from_slice(&line[..w]);
                    }
                }
            } else {
                replicate(&mut out, 2, 2);
            }
        } else if h_in > 0 && v_in > 0 && h_out % h_in == 0 && v_out % v_in == 0 {
            // `int_upsample`: plain replication (any integral ratio).
            replicate(&mut out, h_out / h_in, v_out / v_in);
        } else {
            // `JERR_FRACT_SAMPLE_NOTIMPL`: libjpeg refuses; leave the plane
            // black rather than guess.
        }
        out
    }
}

/// `h2v1_fancy_upsample`: `out[2i] = (3·in[i] + in[i−1] + 1) >> 2`,
/// `out[2i+1] = (3·in[i] + in[i+1] + 2) >> 2`, edges replicated.
fn h2v1_fancy(input: &[u8], out: &mut [u8]) {
    let n = input.len();
    let at = |i: usize| u32::from(input[i]);
    out[0] = input[0];
    out[1] = ((at(0) * 3 + at(1) + 2) >> 2) as u8;
    for i in 1..n - 1 {
        let v = at(i) * 3;
        out[2 * i] = ((v + at(i - 1) + 1) >> 2) as u8;
        out[2 * i + 1] = ((v + at(i + 1) + 2) >> 2) as u8;
    }
    out[2 * (n - 1)] = ((at(n - 1) * 3 + at(n - 2) + 1) >> 2) as u8;
    out[2 * (n - 1) + 1] = input[n - 1];
}

/// `h2v2_fancy_upsample` for one output row: column sums `3·near + far`,
/// then `(3·this + neighbour + 8|7) >> 4` alternating, edges replicated.
fn h2v2_fancy(near: &[u8], far: &[u8], out: &mut [u8]) {
    let n = near.len();
    let colsum = |i: usize| u32::from(near[i]) * 3 + u32::from(far[i]);
    let mut this = colsum(0);
    let mut next = colsum(1);
    out[0] = ((this * 4 + 8) >> 4) as u8;
    out[1] = ((this * 3 + next + 7) >> 4) as u8;
    let mut last = this;
    this = next;
    for i in 1..n - 1 {
        next = colsum(i + 1);
        out[2 * i] = ((this * 3 + last + 8) >> 4) as u8;
        out[2 * i + 1] = ((this * 3 + next + 7) >> 4) as u8;
        last = this;
        this = next;
    }
    out[2 * (n - 1)] = ((this * 3 + last + 8) >> 4) as u8;
    out[2 * (n - 1) + 1] = ((this * 4 + 7) >> 4) as u8;
}

struct YccTables {
    cr_r: [i32; 256],
    cb_b: [i32; 256],
    cr_g: [i32; 256],
    cb_g: [i32; 256],
}

/// `jdcolor.c build_ycc_rgb_table`: 16.16 fixed point, `ONE_HALF` folded
/// into the red/blue tables and the green Cb term.
fn ycc_tables() -> YccTables {
    const SCALEBITS: i32 = 16;
    const ONE_HALF: i32 = 1 << (SCALEBITS - 1);
    let fix = |x: f64| (x * f64::from(1i32 << SCALEBITS) + 0.5) as i32;
    let mut t = YccTables {
        cr_r: [0; 256],
        cb_b: [0; 256],
        cr_g: [0; 256],
        cb_g: [0; 256],
    };
    for i in 0..256i32 {
        let x = i - 128;
        t.cr_r[i as usize] = (fix(1.402) * x + ONE_HALF) >> SCALEBITS;
        t.cb_b[i as usize] = (fix(1.772) * x + ONE_HALF) >> SCALEBITS;
        t.cr_g[i as usize] = -fix(0.71414) * x;
        t.cb_g[i as usize] = -fix(0.34414) * x + ONE_HALF;
    }
    t
}

/// libjpeg's `range_limit` for the colour converter: clamp to 0..=255.
fn range_limit(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// The post-IDCT range-limit table (`prepare_range_limit_table`), indexed by
/// the descaled value `& 1023`: identity around zero (shifted by 128), 255
/// above, 0 below, and the wrap-around segments for out-of-range garbage.
fn idct_range_limit(x: i32) -> u8 {
    let i = (x & 1023) as usize;
    match i {
        0..=127 => (128 + i) as u8,
        128..=511 => 255,
        512..=895 => 0,
        _ => (i - 896) as u8,
    }
}

/// The IDCT for a block at `DCT_scaled_size` `dct` (`jddctmgr.c`: 8 →
/// `jpeg_idct_islow`, 4/2/1 → the `jidctred.c` reduced-size transforms,
/// which always use the islow-style dequantization), writing the `dct` ×
/// `dct` output of block `bi` into a plane `bw` blocks wide.
fn idct(dct: usize, coef: &[i16; 64], q: &[u16; 64], samples: &mut [u8], bi: usize, bw: usize) {
    match dct {
        8 => idct_islow(coef, q, samples, bi, bw),
        4 => idct_4x4(coef, q, samples, bi, bw),
        2 => idct_2x2(coef, q, samples, bi, bw),
        _ => idct_1x1(coef, q, samples, bi, bw),
    }
}

const CONST_BITS: i32 = 13;
const PASS1_BITS: i32 = 2;

#[inline(always)]
fn descale(x: i32, n: i32) -> i32 {
    x.wrapping_add(1 << (n - 1)) >> n
}

#[inline(always)]
fn mul(a: i32, b: i32) -> i32 {
    a.wrapping_mul(b)
}

/// `jpeg_idct_4x4` (jidctred.c): a 4×4 output from the 8×8 block, column 4
/// never examined.
fn idct_4x4(coef: &[i16; 64], q: &[u16; 64], samples: &mut [u8], bi: usize, bw: usize) {
    const FIX_0_211164243: i32 = 1730;
    const FIX_0_509795579: i32 = 4176;
    const FIX_0_601344887: i32 = 4926;
    const FIX_0_765366865: i32 = 6270;
    const FIX_0_899976223: i32 = 7373;
    const FIX_1_061594337: i32 = 8697;
    const FIX_1_451774981: i32 = 11893;
    const FIX_1_847759065: i32 = 15137;
    const FIX_2_172734803: i32 = 17799;
    const FIX_2_562915447: i32 = 20995;
    let dq = |k: usize| i32::from(coef[k]).wrapping_mul(i32::from(q[k]));
    let mut ws = [0i32; 32]; // 4 rows × 8 columns
    for col in 0..8 {
        if col == 4 {
            continue;
        }
        if [1, 2, 3, 5, 6, 7].iter().all(|&r| coef[r * 8 + col] == 0) {
            let dc = dq(col) << PASS1_BITS;
            for r in 0..4 {
                ws[r * 8 + col] = dc;
            }
            continue;
        }
        let tmp0 = dq(col) << (CONST_BITS + 1);
        let z2 = dq(2 * 8 + col);
        let z3 = dq(6 * 8 + col);
        let tmp2 = mul(z2, FIX_1_847759065).wrapping_add(mul(z3, -FIX_0_765366865));
        let tmp10 = tmp0.wrapping_add(tmp2);
        let tmp12 = tmp0.wrapping_sub(tmp2);
        let z1 = dq(7 * 8 + col);
        let z2 = dq(5 * 8 + col);
        let z3 = dq(3 * 8 + col);
        let z4 = dq(8 + col);
        let tmp0 = mul(z1, -FIX_0_211164243)
            .wrapping_add(mul(z2, FIX_1_451774981))
            .wrapping_add(mul(z3, -FIX_2_172734803))
            .wrapping_add(mul(z4, FIX_1_061594337));
        let tmp2 = mul(z1, -FIX_0_509795579)
            .wrapping_add(mul(z2, -FIX_0_601344887))
            .wrapping_add(mul(z3, FIX_0_899976223))
            .wrapping_add(mul(z4, FIX_2_562915447));
        let n = CONST_BITS - PASS1_BITS + 1;
        ws[col] = descale(tmp10.wrapping_add(tmp2), n);
        ws[3 * 8 + col] = descale(tmp10.wrapping_sub(tmp2), n);
        ws[8 + col] = descale(tmp12.wrapping_add(tmp0), n);
        ws[2 * 8 + col] = descale(tmp12.wrapping_sub(tmp0), n);
    }
    let stride = bw * 4;
    let (bx, by) = (bi % bw, bi / bw);
    for row in 0..4 {
        let w = &ws[row * 8..row * 8 + 8];
        let off = (by * 4 + row) * stride + bx * 4;
        let out = &mut samples[off..off + 4];
        if [1, 2, 3, 5, 6, 7].iter().all(|&k| w[k] == 0) {
            out.fill(idct_range_limit(descale(w[0], PASS1_BITS + 3)));
            continue;
        }
        let tmp0 = w[0] << (CONST_BITS + 1);
        let tmp2 = mul(w[2], FIX_1_847759065).wrapping_add(mul(w[6], -FIX_0_765366865));
        let tmp10 = tmp0.wrapping_add(tmp2);
        let tmp12 = tmp0.wrapping_sub(tmp2);
        let (z1, z2, z3, z4) = (w[7], w[5], w[3], w[1]);
        let tmp0 = mul(z1, -FIX_0_211164243)
            .wrapping_add(mul(z2, FIX_1_451774981))
            .wrapping_add(mul(z3, -FIX_2_172734803))
            .wrapping_add(mul(z4, FIX_1_061594337));
        let tmp2 = mul(z1, -FIX_0_509795579)
            .wrapping_add(mul(z2, -FIX_0_601344887))
            .wrapping_add(mul(z3, FIX_0_899976223))
            .wrapping_add(mul(z4, FIX_2_562915447));
        let n = CONST_BITS + PASS1_BITS + 3 + 1;
        out[0] = idct_range_limit(descale(tmp10.wrapping_add(tmp2), n));
        out[3] = idct_range_limit(descale(tmp10.wrapping_sub(tmp2), n));
        out[1] = idct_range_limit(descale(tmp12.wrapping_add(tmp0), n));
        out[2] = idct_range_limit(descale(tmp12.wrapping_sub(tmp0), n));
    }
}

/// `jpeg_idct_2x2` (jidctred.c): columns 2, 4, 6 never examined.
fn idct_2x2(coef: &[i16; 64], q: &[u16; 64], samples: &mut [u8], bi: usize, bw: usize) {
    const FIX_0_720959822: i32 = 5906;
    const FIX_0_850430095: i32 = 6967;
    const FIX_1_272758580: i32 = 10426;
    const FIX_3_624509785: i32 = 29692;
    let dq = |k: usize| i32::from(coef[k]).wrapping_mul(i32::from(q[k]));
    let mut ws = [0i32; 16]; // 2 rows × 8 columns
    for col in [0usize, 1, 3, 5, 7] {
        if [1, 3, 5, 7].iter().all(|&r| coef[r * 8 + col] == 0) {
            let dc = dq(col) << PASS1_BITS;
            ws[col] = dc;
            ws[8 + col] = dc;
            continue;
        }
        let tmp10 = dq(col) << (CONST_BITS + 2);
        let tmp0 = mul(dq(7 * 8 + col), -FIX_0_720959822)
            .wrapping_add(mul(dq(5 * 8 + col), FIX_0_850430095))
            .wrapping_add(mul(dq(3 * 8 + col), -FIX_1_272758580))
            .wrapping_add(mul(dq(8 + col), FIX_3_624509785));
        let n = CONST_BITS - PASS1_BITS + 2;
        ws[col] = descale(tmp10.wrapping_add(tmp0), n);
        ws[8 + col] = descale(tmp10.wrapping_sub(tmp0), n);
    }
    let stride = bw * 2;
    let (bx, by) = (bi % bw, bi / bw);
    for row in 0..2 {
        let w = &ws[row * 8..row * 8 + 8];
        let off = (by * 2 + row) * stride + bx * 2;
        let out = &mut samples[off..off + 2];
        if [1, 3, 5, 7].iter().all(|&k| w[k] == 0) {
            out.fill(idct_range_limit(descale(w[0], PASS1_BITS + 3)));
            continue;
        }
        let tmp10 = w[0] << (CONST_BITS + 2);
        let tmp0 = mul(w[7], -FIX_0_720959822)
            .wrapping_add(mul(w[5], FIX_0_850430095))
            .wrapping_add(mul(w[3], -FIX_1_272758580))
            .wrapping_add(mul(w[1], FIX_3_624509785));
        let n = CONST_BITS + PASS1_BITS + 3 + 2;
        out[0] = idct_range_limit(descale(tmp10.wrapping_add(tmp0), n));
        out[1] = idct_range_limit(descale(tmp10.wrapping_sub(tmp0), n));
    }
}

/// `jpeg_idct_1x1`: the DC term, one eighth.
fn idct_1x1(coef: &[i16; 64], q: &[u16; 64], samples: &mut [u8], bi: usize, bw: usize) {
    let dc = descale(i32::from(coef[0]).wrapping_mul(i32::from(q[0])), 3);
    let (bx, by) = (bi % bw, bi / bw);
    samples[by * bw + bx] = idct_range_limit(dc);
}

/// `jpeg_idct_islow` (jidctint.c): the accurate integer inverse DCT with its
/// exact fixed-point constants and descaling, writing the dequantized block
/// `bi` of a `bw`-blocks-wide plane.
fn idct_islow(coef: &[i16; 64], q: &[u16; 64], samples: &mut [u8], bi: usize, bw: usize) {
    const FIX_0_298631336: i32 = 2446;
    const FIX_0_390180644: i32 = 3196;
    const FIX_0_541196100: i32 = 4433;
    const FIX_0_765366865: i32 = 6270;
    const FIX_0_899976223: i32 = 7373;
    const FIX_1_175875602: i32 = 9633;
    const FIX_1_501321110: i32 = 12299;
    const FIX_1_847759065: i32 = 15137;
    const FIX_1_961570560: i32 = 16069;
    const FIX_2_053119869: i32 = 16819;
    const FIX_2_562915447: i32 = 20995;
    const FIX_3_072711026: i32 = 25172;

    let dq = |k: usize| i32::from(coef[k]).wrapping_mul(i32::from(q[k]));
    let mut ws = [0i32; 64];

    // Pass 1: columns.
    for col in 0..8 {
        if (1..8).all(|r| coef[r * 8 + col] == 0) {
            let dc = dq(col) << PASS1_BITS;
            for r in 0..8 {
                ws[r * 8 + col] = dc;
            }
            continue;
        }
        let z2 = dq(2 * 8 + col);
        let z3 = dq(6 * 8 + col);
        let z1 = mul(z2.wrapping_add(z3), FIX_0_541196100);
        let tmp2 = z1.wrapping_add(mul(z3, -FIX_1_847759065));
        let tmp3 = z1.wrapping_add(mul(z2, FIX_0_765366865));
        let z2 = dq(col);
        let z3 = dq(4 * 8 + col);
        let tmp0 = z2.wrapping_add(z3) << CONST_BITS;
        let tmp1 = z2.wrapping_sub(z3) << CONST_BITS;
        let tmp10 = tmp0.wrapping_add(tmp3);
        let tmp13 = tmp0.wrapping_sub(tmp3);
        let tmp11 = tmp1.wrapping_add(tmp2);
        let tmp12 = tmp1.wrapping_sub(tmp2);

        let mut tmp0 = dq(7 * 8 + col);
        let mut tmp1 = dq(5 * 8 + col);
        let mut tmp2 = dq(3 * 8 + col);
        let mut tmp3 = dq(8 + col);
        let z1 = tmp0.wrapping_add(tmp3);
        let z2 = tmp1.wrapping_add(tmp2);
        let z3 = tmp0.wrapping_add(tmp2);
        let z4 = tmp1.wrapping_add(tmp3);
        let z5 = mul(z3.wrapping_add(z4), FIX_1_175875602);
        tmp0 = mul(tmp0, FIX_0_298631336);
        tmp1 = mul(tmp1, FIX_2_053119869);
        tmp2 = mul(tmp2, FIX_3_072711026);
        tmp3 = mul(tmp3, FIX_1_501321110);
        let z1 = mul(z1, -FIX_0_899976223);
        let z2 = mul(z2, -FIX_2_562915447);
        let z3 = mul(z3, -FIX_1_961570560).wrapping_add(z5);
        let z4 = mul(z4, -FIX_0_390180644).wrapping_add(z5);
        tmp0 = tmp0.wrapping_add(z1).wrapping_add(z3);
        tmp1 = tmp1.wrapping_add(z2).wrapping_add(z4);
        tmp2 = tmp2.wrapping_add(z2).wrapping_add(z3);
        tmp3 = tmp3.wrapping_add(z1).wrapping_add(z4);

        let n = CONST_BITS - PASS1_BITS;
        ws[col] = descale(tmp10.wrapping_add(tmp3), n);
        ws[7 * 8 + col] = descale(tmp10.wrapping_sub(tmp3), n);
        ws[8 + col] = descale(tmp11.wrapping_add(tmp2), n);
        ws[6 * 8 + col] = descale(tmp11.wrapping_sub(tmp2), n);
        ws[2 * 8 + col] = descale(tmp12.wrapping_add(tmp1), n);
        ws[5 * 8 + col] = descale(tmp12.wrapping_sub(tmp1), n);
        ws[3 * 8 + col] = descale(tmp13.wrapping_add(tmp0), n);
        ws[4 * 8 + col] = descale(tmp13.wrapping_sub(tmp0), n);
    }

    // Pass 2: rows.
    let stride = bw * 8;
    let (bx, by) = (bi % bw, bi / bw);
    for row in 0..8 {
        let w = &ws[row * 8..row * 8 + 8];
        let out_off = (by * 8 + row) * stride + bx * 8;
        let out = &mut samples[out_off..out_off + 8];
        let n = CONST_BITS + PASS1_BITS + 3;
        if w[1..].iter().all(|&v| v == 0) {
            let dc = idct_range_limit(descale(w[0], PASS1_BITS + 3));
            out.fill(dc);
            continue;
        }
        let z2 = w[2];
        let z3 = w[6];
        let z1 = mul(z2.wrapping_add(z3), FIX_0_541196100);
        let tmp2 = z1.wrapping_add(mul(z3, -FIX_1_847759065));
        let tmp3 = z1.wrapping_add(mul(z2, FIX_0_765366865));
        let tmp0 = w[0].wrapping_add(w[4]) << CONST_BITS;
        let tmp1 = w[0].wrapping_sub(w[4]) << CONST_BITS;
        let tmp10 = tmp0.wrapping_add(tmp3);
        let tmp13 = tmp0.wrapping_sub(tmp3);
        let tmp11 = tmp1.wrapping_add(tmp2);
        let tmp12 = tmp1.wrapping_sub(tmp2);

        let mut tmp0 = w[7];
        let mut tmp1 = w[5];
        let mut tmp2 = w[3];
        let mut tmp3 = w[1];
        let z1 = tmp0.wrapping_add(tmp3);
        let z2 = tmp1.wrapping_add(tmp2);
        let z3 = tmp0.wrapping_add(tmp2);
        let z4 = tmp1.wrapping_add(tmp3);
        let z5 = mul(z3.wrapping_add(z4), FIX_1_175875602);
        tmp0 = mul(tmp0, FIX_0_298631336);
        tmp1 = mul(tmp1, FIX_2_053119869);
        tmp2 = mul(tmp2, FIX_3_072711026);
        tmp3 = mul(tmp3, FIX_1_501321110);
        let z1 = mul(z1, -FIX_0_899976223);
        let z2 = mul(z2, -FIX_2_562915447);
        let z3 = mul(z3, -FIX_1_961570560).wrapping_add(z5);
        let z4 = mul(z4, -FIX_0_390180644).wrapping_add(z5);
        tmp0 = tmp0.wrapping_add(z1).wrapping_add(z3);
        tmp1 = tmp1.wrapping_add(z2).wrapping_add(z4);
        tmp2 = tmp2.wrapping_add(z2).wrapping_add(z3);
        tmp3 = tmp3.wrapping_add(z1).wrapping_add(z4);

        out[0] = idct_range_limit(descale(tmp10.wrapping_add(tmp3), n));
        out[7] = idct_range_limit(descale(tmp10.wrapping_sub(tmp3), n));
        out[1] = idct_range_limit(descale(tmp11.wrapping_add(tmp2), n));
        out[6] = idct_range_limit(descale(tmp11.wrapping_sub(tmp2), n));
        out[2] = idct_range_limit(descale(tmp12.wrapping_add(tmp1), n));
        out[5] = idct_range_limit(descale(tmp12.wrapping_sub(tmp1), n));
        out[3] = idct_range_limit(descale(tmp13.wrapping_add(tmp0), n));
        out[4] = idct_range_limit(descale(tmp13.wrapping_sub(tmp0), n));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dc_only_block_is_flat() {
        let mut coef = [0i16; 64];
        coef[0] = 8; // × q 1 → 8/8 = +1 over mid-gray
        let q = [1u16; 64];
        let mut samples = vec![0u8; 64];
        idct_islow(&coef, &q, &mut samples, 0, 1);
        assert!(samples.iter().all(|&v| v == 129), "{samples:?}");
    }

    #[test]
    fn colour_tables_match_libjpeg_constants() {
        let t = ycc_tables();
        // Cr = 255 → x = 127 → round(1.402 · 127) = 178; Cb = 0 → x = −128 → round(1.772 · −128) = −227.
        assert_eq!(t.cr_r[255], 178);
        assert_eq!(t.cb_b[0], -227);
        assert_eq!(range_limit(300), 255);
        assert_eq!(idct_range_limit(-5), 123);
        assert_eq!(idct_range_limit(200), 255);
        assert_eq!(idct_range_limit(-300), 0);
    }
}
