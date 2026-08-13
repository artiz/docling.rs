//! Lotus 1-2-3 / Symphony / MS Works spreadsheets (#216) —
//! the DOS-era record-stream formats, parsed natively (docling reads none of
//! them). One backend, content-sniffed on the BOF record so a misnamed file
//! still converts; record layouts follow Gnumeric's lotus-123 importer, the
//! richest open documentation of the family:
//!
//! - **WK1/WKS/Symphony** (`.wk1`/`.wk2`/`.wks`/`.wrk`, BOF versions
//!   0x0404–0x0406): `[u16 opcode][u16 len]` records; INTEGER/NUMBER/LABEL/
//!   FORMULA cells address `[fmt u8][col u16][row u16]`. Formula results are
//!   the cached number — a NaN-boxed cache means the value is the following
//!   STRING record (or an error).
//! - **WK3/WK4/123** (BOF 0x1000–0x1005): cells address
//!   `[row u16][sheet u8][col u8]`; numbers arrive as 10-byte extended
//!   floats, packed u32s, or the SMALLNUM integer encoding; string formula
//!   results land in a separate FORMULASTRING record that overwrites the
//!   placeholder the cached value left behind.
//! - **MS Works v3 spreadsheet** (`.wks` again — BOF *opcode* 0xFF instead
//!   of 0x00): Lotus-shaped records with `[col u16][row u16][fmt u16]` cell
//!   addressing plus Works' packed-f32 SMALL_FLOAT.
//!
//! All of these are sheet snapshots, so each sheet runs through the same
//! flood-fill region splitting as ODS sheets (`emit_sheet_regions`) — a
//! `.wk1` holding a sheet's data converts to the same tables as the `.ods`.
//! Labels decode high bytes as cp1252 through the shared table (LMBCS
//! multi-byte groups degrade to their base character; the fixtures — and
//! virtually all surviving files — are ASCII).

use std::collections::{BTreeMap, HashMap};

use crate::backend::odf::emit_sheet_regions;
use crate::backend::rtf::decode_byte;
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;
use docling_core::DoclingDocument;

pub struct LotusBackend;

impl DeclarativeBackend for LotusBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let d = &source.bytes;
        if d.len() < 6 {
            return Err(ConversionError::Parse("lotus: file too short".into()));
        }
        let opcode = u16::from_le_bytes([d[0], d[1]]);
        let version = u16::from_le_bytes([d[4], d[5]]);
        let mut doc = DoclingDocument::new(&source.name);
        let sheets = match (opcode, version) {
            // 1-2-3 rel 1A / Symphony / rel 2.x.
            (0x0000, 0x0404..=0x0406) => read_old(d),
            // 1-2-3 rel 3+ / SmartSuite (.wk3/.wk4/.123).
            (0x0000, 0x1000..=0x1005) => read_new(d),
            // MS Works spreadsheet: same framing, BOF opcode 0xFF.
            (0x00ff, 0x0404) => read_works(d),
            _ => {
                return Err(ConversionError::Parse(
                    "lotus: no Lotus/Works BOF signature".into(),
                ))
            }
        };
        for cells in sheets {
            emit_sheet_regions(&cells, &mut doc);
        }
        Ok(doc)
    }
}

type Grid = HashMap<(usize, usize), String>;

/// `[u16 opcode][u16 len][payload]` little-endian record stream — the framing
/// every family member shares.
struct Records<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Records<'a> {
    fn new(d: &'a [u8]) -> Self {
        Records { d, pos: 0 }
    }

    /// The next record's opcode without consuming it (formula string results
    /// peek for their STRING record).
    fn peek_opcode(&self) -> Option<u16> {
        (self.pos + 4 <= self.d.len())
            .then(|| u16::from_le_bytes([self.d[self.pos], self.d[self.pos + 1]]))
    }
}

impl<'a> Iterator for Records<'a> {
    type Item = (u16, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos + 4 > self.d.len() {
            return None;
        }
        let op = u16::from_le_bytes([self.d[self.pos], self.d[self.pos + 1]]);
        let len = u16::from_le_bytes([self.d[self.pos + 2], self.d[self.pos + 3]]) as usize;
        self.pos += 4;
        // A truncated final record ends the stream rather than erroring — the
        // rest of the file already parsed.
        let payload = self.d.get(self.pos..self.pos + len)?;
        self.pos += len;
        Some((op, payload))
    }
}

fn le16(d: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([d[at], d[at + 1]])
}

fn f64_at(d: &[u8], at: usize) -> f64 {
    f64::from_le_bytes(d[at..at + 8].try_into().unwrap())
}

/// Spreadsheet display form: integers without a decimal point, everything
/// else the shortest `f64` form (same rule as the DIF/SYLK backends).
fn number_text(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// NUL-terminated label bytes → text. ASCII passes through, high bytes decode
/// as cp1252; LMBCS group-switch leaders (0x01–0x1F) are dropped.
fn label_text(d: &[u8]) -> String {
    d.iter()
        .take_while(|&&b| b != 0)
        .filter(|&&b| b >= 0x20 || b == b'\t')
        .map(|&b| decode_byte(b, 1252))
        .collect()
}

fn insert(grid: &mut Grid, row: usize, col: usize, text: String) {
    if !text.is_empty() {
        grid.insert((row, col), text);
    }
}

/// The cached result of a FORMULA record is NaN-boxed when the real result is
/// a string (in the STRING record that follows) or an error.
fn nan_boxed(hi16: u16) -> bool {
    hi16 & 0x7ff8 == 0x7ff0
}

// ------------------------------------------------------- WK1 / WKS / WK2

/// Cell records address `[fmt u8][col u16 @1][row u16 @3]`, value from
/// offset 5. Each BOF starts a sheet (Symphony files can hold several).
fn read_old(d: &[u8]) -> Vec<Grid> {
    let mut sheets: Vec<Grid> = Vec::new();
    let mut records = Records::new(d);
    while let Some((op, p)) = records.next() {
        // Cell records before any BOF would be malformed; index safely.
        let grid = |sheets: &mut Vec<Grid>| -> usize {
            if sheets.is_empty() {
                sheets.push(Grid::new());
            }
            sheets.len() - 1
        };
        match op {
            0x00 => sheets.push(Grid::new()),
            0x01 => {} // EOF — a following BOF would start the next sheet
            // INTEGER: i16 value.
            0x0d if p.len() >= 7 => {
                let (col, row) = (le16(p, 1) as usize, le16(p, 3) as usize);
                let v = i16::from_le_bytes([p[5], p[6]]);
                let s = grid(&mut sheets);
                insert(&mut sheets[s], row, col, v.to_string());
            }
            // NUMBER: f64 value.
            0x0e if p.len() >= 13 => {
                let (col, row) = (le16(p, 1) as usize, le16(p, 3) as usize);
                let s = grid(&mut sheets);
                insert(&mut sheets[s], row, col, number_text(f64_at(p, 5)));
            }
            // LABEL: alignment prefix at 5 (' " ^ \), text from 6.
            0x0f if p.len() >= 7 => {
                let (col, row) = (le16(p, 1) as usize, le16(p, 3) as usize);
                let s = grid(&mut sheets);
                insert(&mut sheets[s], row, col, label_text(&p[6..]));
            }
            // FORMULA: cached f64 at 5; NaN-boxed cache → the following
            // STRING record carries the text result (missing → an error).
            0x10 if p.len() >= 15 => {
                let (col, row) = (le16(p, 1) as usize, le16(p, 3) as usize);
                let text = if nan_boxed(le16(p, 11)) {
                    if records.peek_opcode() == Some(0x33) {
                        let (_, sp) = records.next().unwrap();
                        // STRING shares the cell-record shape; text from 5.
                        if sp.len() > 5 {
                            label_text(&sp[5..])
                        } else {
                            String::new()
                        }
                    } else {
                        "#VALUE!".to_string()
                    }
                } else {
                    number_text(f64_at(p, 5))
                };
                let s = grid(&mut sheets);
                insert(&mut sheets[s], row, col, text);
            }
            _ => {}
        }
    }
    sheets
}

// --------------------------------------------------------- WK3 / WK4 / 123

/// Cell records address `[row u16 @0][sheet u8 @2][col u8 @3]`, value from
/// offset 4. Sheets are indexed, not sequential — collect into a map.
fn read_new(d: &[u8]) -> Vec<Grid> {
    let mut sheets: BTreeMap<u8, Grid> = BTreeMap::new();
    for (op, p) in Records::new(d) {
        if op == 0x01 {
            break; // EOF
        }
        if p.len() < 4 {
            continue;
        }
        let (row, sheet, col) = (le16(p, 0) as usize, p[2], p[3] as usize);
        let grid = sheets.entry(sheet).or_default();
        match op {
            // ERRCELL / NACELL.
            0x14 => insert(grid, row, col, "#VALUE!".to_string()),
            0x15 => insert(grid, row, col, "#N/A".to_string()),
            // LABEL2: alignment prefix at 4, text from 5.
            0x16 if p.len() >= 6 => insert(grid, row, col, label_text(&p[5..])),
            // EXTENDED_FLOAT: 10-byte long double.
            0x17 if p.len() >= 14 => {
                if let Some(text) = treal_text(&p[4..14]) {
                    insert(grid, row, col, text);
                }
            }
            // SMALLNUM: packed 16-bit integer/decimal.
            0x18 if p.len() >= 6 => {
                let v = i16::from_le_bytes([p[4], p[5]]);
                insert(grid, row, col, smallnum_text(v));
            }
            // FORMULA3: 10-byte cached result, bytecode after — the cache is
            // the value; a string result arrives via FORMULASTRING below.
            0x19 if p.len() >= 15 => {
                if let Some(text) = treal_text(&p[4..14]) {
                    insert(grid, row, col, text);
                }
            }
            // FORMULASTRING: the text result of a string formula, replacing
            // the empty placeholder its FORMULA3 cache left.
            0x1a if p.len() >= 5 => insert(grid, row, col, label_text(&p[4..])),
            // PACKED_NUMBER: u32 → value*10^±exp.
            0x25 if p.len() == 8 => {
                let u = u32::from_le_bytes([p[4], p[5], p[6], p[7]]);
                insert(grid, row, col, packed_number_text(u));
            }
            // NUMBER2: plain f64 (wk4).
            0x27 if p.len() >= 12 => insert(grid, row, col, number_text(f64_at(p, 4))),
            // FORMULA2: f64 cached result, bytecode after (wk4).
            0x28 if p.len() >= 13 => insert(grid, row, col, number_text(f64_at(p, 4))),
            _ => {}
        }
    }
    sheets.into_values().collect()
}

/// 10-byte extended float: u64 mantissa + u16 sign/exponent, with reserved
/// bit patterns for empty/error/string-pending cells. `None` = leave the
/// cell alone (empty, or its string is coming in a FORMULASTRING record).
fn treal_text(b: &[u8]) -> Option<String> {
    if b[9] == 0xff && b[8] == 0xff {
        return match b[7] {
            0xc0 => Some("#VALUE!".to_string()),
            0xd0 => Some("#N/A".to_string()),
            // 0x00 = empty, 0xe0 = string result pending.
            _ => None,
        };
    }
    let mant = u64::from_le_bytes(b[..8].try_into().unwrap());
    let signexp = u16::from_le_bytes([b[8], b[9]]);
    let exp = (signexp & 0x7fff) as i32 - 16383;
    let v = (mant as f64) * 2f64.powi(exp - 63);
    let v = if signexp & 0x8000 != 0 { -v } else { v };
    Some(number_text(v))
}

/// SMALLNUM: even = value>>1; odd = mantissa (>>4) times a table factor,
/// negative factors meaning division.
fn smallnum_text(v: i16) -> String {
    if v & 1 != 0 {
        const FACTORS: [i32; 8] = [5000, 500, -20, -200, -2000, -20000, -16, -64];
        let f = FACTORS[((v >> 1) & 7) as usize];
        let mant = (v >> 4) as i32;
        if f > 0 {
            ((f as i64) * (mant as i64)).to_string()
        } else {
            number_text(mant as f64 / -f as f64)
        }
    } else {
        (v >> 1).to_string()
    }
}

/// PACKED_NUMBER: bits 6.. = magnitude, bit 5 = sign, bit 4 = divide (else
/// multiply) by 10^(bits 0–3).
fn packed_number_text(u: u32) -> String {
    let mut v = (u >> 6) as f64;
    if u & 0x20 != 0 {
        v = -v;
    }
    let p = 10f64.powi((u & 15) as i32);
    number_text(if u & 0x10 != 0 { v / p } else { v * p })
}

// ------------------------------------------------------------ MS Works v3

/// Works kept the Lotus opcodes but re-shaped cell addressing to
/// `[col u16 @0][row u16 @2][fmt u16 @4]`, value from offset 6.
fn read_works(d: &[u8]) -> Vec<Grid> {
    let mut sheets: Vec<Grid> = Vec::new();
    let mut records = Records::new(d);
    while let Some((op, p)) = records.next() {
        let grid = |sheets: &mut Vec<Grid>| -> usize {
            if sheets.is_empty() {
                sheets.push(Grid::new());
            }
            sheets.len() - 1
        };
        match op {
            0xff => sheets.push(Grid::new()),
            0x01 => {}
            // NUMBER: f64 at 6.
            0x0e if p.len() >= 14 => {
                let (col, row) = (le16(p, 0) as usize, le16(p, 2) as usize);
                let s = grid(&mut sheets);
                insert(&mut sheets[s], row, col, number_text(f64_at(p, 6)));
            }
            // LABEL: text from 6, no alignment prefix.
            0x0f if p.len() >= 8 => {
                let (col, row) = (le16(p, 0) as usize, le16(p, 2) as usize);
                let s = grid(&mut sheets);
                insert(&mut sheets[s], row, col, label_text(&p[6..]));
            }
            // FORMULA: cached f64 at 6; NaN-boxed → STRING record (text
            // from 6) or an error, as in WK1.
            0x10 if p.len() >= 16 => {
                let (col, row) = (le16(p, 0) as usize, le16(p, 2) as usize);
                let text = if nan_boxed(le16(p, 12)) {
                    if records.peek_opcode() == Some(0x33) {
                        let (_, sp) = records.next().unwrap();
                        if sp.len() > 6 {
                            label_text(&sp[6..])
                        } else {
                            String::new()
                        }
                    } else {
                        "#VALUE!".to_string()
                    }
                } else {
                    number_text(f64_at(p, 6))
                };
                let s = grid(&mut sheets);
                insert(&mut sheets[s], row, col, text);
            }
            // WORKS_SMALL_FLOAT: f32 packed into a u32 with a /100 flag in
            // bit 0 (col is a single byte here).
            0x545b if p.len() >= 10 => {
                let (col, row) = (p[0] as usize, le16(p, 2) as usize);
                let raw = u32::from_le_bytes([p[6], p[7], p[8], p[9]]);
                let flag = raw & 1 != 0;
                let bits = (raw & 0xfc00_0000) | ((raw & 0x03ff_fffe) << 3);
                let mut v = f32::from_bits(bits) as f64;
                if flag {
                    v /= 100.0;
                }
                let s = grid(&mut sheets);
                insert(&mut sheets[s], row, col, number_text(v));
            }
            _ => {}
        }
    }
    sheets
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SMALLNUM factor table drives both the integer and the fraction
    /// encodings — the WK3/WK4 compact number path.
    #[test]
    fn smallnum_decodes_both_encodings() {
        assert_eq!(smallnum_text(950 << 1), "950");
        assert_eq!(smallnum_text(-3 << 1), "-3");
        // odd: mantissa 18, factor index 3 (-200) → 18/200
        assert_eq!(smallnum_text((18 << 4) | (3 << 1) | 1), "0.09");
        // factor index 0 (5000) multiplies
        assert_eq!(smallnum_text((2 << 4) | 1), "10000");
    }

    #[test]
    fn packed_number_scales_by_powers_of_ten() {
        assert_eq!(packed_number_text((71 << 6) | 1), "710");
        assert_eq!(packed_number_text((16 << 6) | 0x10 | 2), "0.16");
        assert_eq!(packed_number_text((5 << 6) | 0x20), "-5");
    }

    /// 10-byte extended floats carry reserved patterns for error/NA/pending
    /// cells alongside real values.
    #[test]
    fn treal_reads_values_and_markers() {
        // 1.5 = mantissa 0xC000_0000_0000_0000, exponent 16383
        let mut b = [0u8; 10];
        b[..8].copy_from_slice(&0xC000_0000_0000_0000u64.to_le_bytes());
        b[8..].copy_from_slice(&16383u16.to_le_bytes());
        assert_eq!(treal_text(&b).as_deref(), Some("1.5"));
        let na = [0, 0, 0, 0, 0, 0, 0, 0xd0, 0xff, 0xff];
        assert_eq!(treal_text(&na).as_deref(), Some("#N/A"));
        let pending = [0, 0, 0, 0, 0, 0, 0, 0xe0, 0xff, 0xff];
        assert_eq!(treal_text(&pending), None);
    }
}
