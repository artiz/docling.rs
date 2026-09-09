//! Quattro Pro spreadsheets (#216) — a docling.rs extension (docling reaches
//! Quattro Pro only through LibreOffice's libwps import filter). Every
//! generation is a `[u16 type][u16 len]` record stream, but the cell records
//! moved with each one, which is why the Lotus reader (`lotus.rs`) could not
//! take them; the layouts below follow libwps' `QuattroDos`, `Quattro` and
//! `Quattro9` parsers, checked against the Open Preservation format-corpus
//! samples:
//!
//! - **Quattro Pro for DOS 1–4 (`.wq1`, BOF version 0x5120) and 5 (`.wq2`,
//!   0x5121)**: cells 0x0C–0x10 address `[fmt u8]` (wq1 only) `[col u8]
//!   [sheet u8][row i16]` (+ `[style u16]` in wq2), then data: i16, f64,
//!   `[align][pascal string]` labels, or an f64 formula cache followed by the
//!   bytecode. Formula string results arrive as 0x33 records with their own
//!   address and a pascal string. Labels are code page 437.
//! - **Quattro Pro for Windows 1/5/6 (`.wb1`, `.wb2`; BOF 0x1001/0x1002) and
//!   7/8 (`.wb3`, BOF 0x1007 inside the OLE `PerfectOffice_MAIN` stream)**:
//!   record types carry a high flag bit (`& 0x7FFF`); cells address `[col
//!   u8][sheet u8][row u16][style u16]`, labels are `[align][C string]`,
//!   formulas an f64 cache + `u16` state + bytecode. Code page 1252.
//! - **Quattro Pro 9–X9 (`.qpw`, OLE `NativeContent_MAIN` opening `01 00 0E
//!   00 "QPW9"`)**: zones `[u16 id][u16 len]` (`u32 len` when bit 15 of the
//!   id is set); zone 0x407 is the document string table (`u32 n`, two
//!   `u32`, then `[u16 len][u8 flags][bytes]` entries, an optional
//!   `[u16 size]…` style block when flags bit 1 is set), 0x601 begins a
//!   sheet, 0xA01 a column, 0xC01 is a run of cells down that column (`u32
//!   first row`, `u32 count`, then typed cells: `u8 type` with 0x80 = a
//!   `u16` style follows, 0x40 = a `u16` count of listed values, 0x60 = a
//!   `u16` count of first-value-plus-increment; types 1 empty, 2 `u16`, 3
//!   `i16`, 4 packed 4-byte float, 5 f64, 7 `u32` 1-based string index, 8
//!   formula `f64` + `u16` + `u32`), 0xC02 a formula's string result (`u16
//!   col`, `u32 row`, string).
//!
//! Formulas contribute their cached value (a NaN cache is the empty cell a
//! string result then fills). Each sheet is a snapshot grid and runs through
//! the same flood-fill region splitting as ODS/Lotus sheets.

use std::collections::{BTreeMap, HashMap};

use crate::backend::cfb::CompoundFile;
use crate::backend::doc::cp1252;
use crate::backend::odf::emit_sheet_regions;
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;
use docling_core::DoclingDocument;

pub struct QuattroBackend;

impl DeclarativeBackend for QuattroBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let d = &source.bytes;
        let sheets = if CompoundFile::detect(d) {
            let cfb = CompoundFile::open(d).ok_or_else(|| {
                ConversionError::Parse("quattro: unreadable OLE container".into())
            })?;
            if let Some(main) = cfb.stream("PerfectOffice_MAIN") {
                if main.len() >= 6 && main[..4] == [0, 0, 2, 0] && main[4..6] == [0x07, 0x10] {
                    read_windows(&main)
                } else {
                    return Err(ConversionError::Parse(
                        "quattro: PerfectOffice_MAIN stream without a Quattro Pro BOF".into(),
                    ));
                }
            } else if let Some(main) = cfb.stream("NativeContent_MAIN") {
                if main.len() >= 8 && main[..4] == [1, 0, 0x0e, 0] && &main[4..8] == b"QPW9" {
                    read_qpw(&main)
                } else {
                    return Err(ConversionError::Parse(
                        "quattro: NativeContent_MAIN stream is not a QPW9 spreadsheet".into(),
                    ));
                }
            } else {
                return Err(ConversionError::Parse(
                    "quattro: OLE container without a Quattro Pro stream \
                     (no PerfectOffice_MAIN / NativeContent_MAIN)"
                        .into(),
                ));
            }
        } else {
            if d.len() < 6 {
                return Err(ConversionError::Parse("quattro: file too short".into()));
            }
            let opcode = u16::from_le_bytes([d[0], d[1]]);
            let version = u16::from_le_bytes([d[4], d[5]]);
            match (opcode, version) {
                (0, 0x5120) => read_dos(d, 1),
                (0, 0x5121) => read_dos(d, 2),
                (0, 0x1001 | 0x1002) => read_windows(d),
                _ => {
                    return Err(ConversionError::Parse(
                        "quattro: no Quattro Pro BOF signature".into(),
                    ))
                }
            }
        };
        let mut doc = DoclingDocument::new(&source.name);
        for cells in sheets.into_values() {
            emit_sheet_regions(&cells, &mut doc);
        }
        Ok(doc)
    }
}

type Grid = HashMap<(usize, usize), String>;
type Sheets = BTreeMap<u16, Grid>;

/// `[u16 type][u16 len][payload]` records; `mask` strips the Windows
/// generations' high flag bit. A truncated final record ends the stream.
fn records(d: &[u8], mask: bool) -> impl Iterator<Item = (u16, &[u8])> {
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        if pos + 4 > d.len() {
            return None;
        }
        let mut op = u16::from_le_bytes([d[pos], d[pos + 1]]);
        if mask {
            op &= 0x7fff;
        }
        let len = u16::from_le_bytes([d[pos + 2], d[pos + 3]]) as usize;
        pos += 4;
        let payload = d.get(pos..pos + len)?;
        pos += len;
        Some((op, payload))
    })
}

fn insert(sheets: &mut Sheets, sheet: u16, row: usize, col: usize, text: String) {
    if !text.is_empty() {
        sheets.entry(sheet).or_default().insert((row, col), text);
    }
}

/// Spreadsheet display form: integers without a decimal point, everything
/// else the shortest `f64` form (the DIF/SYLK/Lotus rule).
fn number_text(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// A formula's cached result: NaN means a string (or error) result that a
/// separate string-result record supplies, so the cell stays empty; an
/// infinity is Quattro's `ERR` marker (the sample's `@LOG` of a negative).
fn formula_text(v: f64) -> Option<String> {
    if v.is_nan() {
        None
    } else if v.is_infinite() {
        Some("ERR".to_string())
    } else {
        Some(number_text(v))
    }
}

fn f64_at(d: &[u8], at: usize) -> Option<f64> {
    d.get(at..at + 8)
        .map(|b| f64::from_le_bytes(b.try_into().unwrap()))
}

/// Label bytes → text: NUL ends the string, control bytes drop, high bytes
/// decode through the generation's code page; a carriage return inside a QPW
/// string is a line break libwps also flattens to a space.
fn text(bytes: &[u8], dos: bool) -> String {
    bytes
        .iter()
        .take_while(|&&b| b != 0)
        .filter_map(|&b| match b {
            0x0d => Some(' '),
            b if b < 0x20 => (b == b'\t').then_some(' '),
            b if b < 0x80 => Some(b as char),
            b if dos => Some(CP437[usize::from(b - 0x80)]),
            b => Some(cp1252(b)),
        })
        .collect()
}

/// A `[u8 len][bytes]` pascal string.
fn pascal(d: &[u8], dos: bool) -> String {
    match d.split_first() {
        Some((&n, rest)) => text(&rest[..usize::from(n).min(rest.len())], dos),
        None => String::new(),
    }
}

// --------------------------------------------------------------- DOS 1–5

/// wq1 (`vers` 1): `[fmt][col][sheet][row i16]` + data; wq2 (`vers` 2):
/// `[col][sheet][row i16][style u16]` + data.
fn read_dos(d: &[u8], vers: u8) -> Sheets {
    let mut sheets = Sheets::new();
    let (base, hdr) = if vers == 1 { (1, 5) } else { (0, 6) };
    for (op, p) in records(d, false) {
        if op == 0x01 {
            break;
        }
        if !(matches!(op, 0x0c..=0x10) || op == 0x33) || p.len() < hdr {
            continue;
        }
        let col = usize::from(p[base]);
        let sheet = u16::from(p[base + 1]);
        let row = i16::from_le_bytes([p[base + 2], p[base + 3]]);
        if row < 0 {
            continue;
        }
        let row = row as usize;
        let data = &p[hdr..];
        match op {
            // INTEGER
            0x0d if data.len() >= 2 => {
                let v = i16::from_le_bytes([data[0], data[1]]);
                insert(&mut sheets, sheet, row, col, v.to_string());
            }
            // NUMBER
            0x0e => {
                if let Some(v) = f64_at(data, 0) {
                    insert(&mut sheets, sheet, row, col, number_text(v));
                }
            }
            // LABEL: alignment prefix, pascal string
            0x0f if !data.is_empty() => {
                insert(&mut sheets, sheet, row, col, pascal(&data[1..], true));
            }
            // FORMULA: cached f64 (NaN = a string result; the 0x33 record
            // carries the string)
            0x10 => {
                if let Some(t) = f64_at(data, 0).and_then(formula_text) {
                    insert(&mut sheets, sheet, row, col, t);
                }
            }
            // formula string result: pascal string, no alignment byte
            0x33 => insert(&mut sheets, sheet, row, col, pascal(data, true)),
            _ => {}
        }
    }
    sheets
}

// ---------------------------------------------------------- Windows 1–8

/// `[col][sheet][row u16][style u16]` + data; labels `[align][C string]`.
fn read_windows(d: &[u8]) -> Sheets {
    let mut sheets = Sheets::new();
    for (op, p) in records(d, true) {
        if op == 0x01 {
            break;
        }
        if !(matches!(op, 0x0c..=0x10) || op == 0x33) || p.len() < 6 {
            continue;
        }
        let col = usize::from(p[0]);
        let sheet = u16::from(p[1]);
        let row = usize::from(u16::from_le_bytes([p[2], p[3]]));
        let data = &p[6..];
        match op {
            0x0d if data.len() >= 2 => {
                let v = i16::from_le_bytes([data[0], data[1]]);
                insert(&mut sheets, sheet, row, col, v.to_string());
            }
            0x0e => {
                if let Some(v) = f64_at(data, 0) {
                    insert(&mut sheets, sheet, row, col, number_text(v));
                }
            }
            0x0f | 0x33 if !data.is_empty() => {
                insert(&mut sheets, sheet, row, col, text(&data[1..], false));
            }
            0x10 => {
                if let Some(t) = f64_at(data, 0).and_then(formula_text) {
                    insert(&mut sheets, sheet, row, col, t);
                }
            }
            _ => {}
        }
    }
    sheets
}

// ------------------------------------------------------------- QPW 9–X9

fn u16le(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32le(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// QPW zones: `[u16 id][u16 len]`, or `[u16 id|0x8000][u32 len]`.
fn zones(d: &[u8]) -> impl Iterator<Item = (u16, &[u8])> {
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        let id = u16le(d, pos)?;
        let (len, hdr) = if id & 0x8000 != 0 {
            (u32le(d, pos + 2)? as usize, 6)
        } else {
            (usize::from(u16le(d, pos + 2)?), 4)
        };
        let payload = d.get(pos + hdr..pos + hdr + len)?;
        pos += hdr + len;
        Some((id & 0x7fff, payload))
    })
}

/// A QPW string entry at `at`: `[u16 len][u8 flags][bytes]`, then a
/// `[u16 size]`-prefixed style block when flags bit 1 is set. Returns the
/// text and the offset after the entry.
fn qpw_string(d: &[u8], at: usize) -> Option<(String, usize)> {
    let len = usize::from(u16le(d, at)?);
    let flags = *d.get(at + 2)?;
    let bytes = d.get(at + 3..at + 3 + len)?;
    let mut end = at + 3 + len;
    if flags & 0x02 != 0 {
        let size = usize::from(u16le(d, end)?);
        if size < 6 {
            return None;
        }
        end += size;
    }
    Some((text(bytes, false), end))
}

/// libwps' `readDouble4`: Quattro's packed 4-byte number. Low two bits of
/// the first byte: 2 = a 30-bit integer (`value >> 2`, two's complement at
/// bit 29); otherwise a 20-bit mantissa / 11-bit exponent / sign float,
/// divided by 100 when bit 0 is set.
fn double4(b: [u8; 4]) -> Option<f64> {
    let first = u32::from(b[0]);
    if first & 3 == 2 {
        let raw = u32::from_le_bytes(b);
        let v = i64::from(raw >> 2);
        return Some(if v & 0x2000_0000 != 0 {
            (v - 0x4000_0000) as f64
        } else {
            v as f64
        });
    }
    let mut mant = f64::from(first & 0xfc) / 256.0 + f64::from(b[1]);
    let mant_exp = u32::from(b[2]);
    mant = (mant / 256.0 + f64::from(0x10 + (mant_exp & 0x0f))) / 16.0;
    let mut exp = ((mant_exp & 0xf0) >> 4) as i32 + ((u32::from(b[3]) << 4) as i32);
    let neg = exp & 0x800 != 0;
    exp &= 0x7ff;
    if exp == 0 {
        return (mant > 1.0 - 1e-4).then_some(0.0);
    }
    if exp == 0x7ff {
        return None; // NaN / infinity / text-result marker
    }
    let mut v = mant * 2f64.powi(exp - 0x3ff);
    if neg {
        v = -v;
    }
    if first & 1 != 0 {
        v /= 100.0;
    }
    Some(v)
}

fn read_qpw(d: &[u8]) -> Sheets {
    let mut sheets = Sheets::new();
    let mut strings: Vec<String> = Vec::new();
    let mut sheet: u16 = 0;
    let mut col: usize = 0;
    for (id, p) in zones(d) {
        match id {
            // document strings
            0x407 => {
                let Some(n) = u32le(p, 0) else { continue };
                let mut at = 12;
                for _ in 0..n {
                    let Some((s, end)) = qpw_string(p, at) else {
                        break;
                    };
                    strings.push(s);
                    at = end;
                }
            }
            // begin sheet
            0x601 => sheet = u16le(p, 0).unwrap_or(0),
            // begin column
            0xa01 => col = usize::from(u16le(p, 0).unwrap_or(0)),
            // cells down the current column
            0xc01 => qpw_cells(p, &strings, &mut sheets, sheet, col),
            // formula string result
            0xc02 => {
                let (Some(c), Some(row)) = (u16le(p, 0), u32le(p, 2)) else {
                    continue;
                };
                if let Some((s, _)) = qpw_string(p, 6) {
                    insert(&mut sheets, sheet, row as usize, usize::from(c), s);
                }
            }
            _ => {}
        }
    }
    sheets
}

/// A 0xC01 zone: `u32 first row`, `u32 cell count`, then typed cells (see
/// the module docs). Lists give one value per row; increments give
/// `first + k*step`.
fn qpw_cells(p: &[u8], strings: &[String], sheets: &mut Sheets, sheet: u16, col: usize) {
    let (Some(first_row), Some(_count)) = (u32le(p, 0), u32le(p, 4)) else {
        return;
    };
    let mut row = first_row as usize;
    let mut at = 8;
    while at < p.len() {
        let mut typ = p[at];
        at += 1;
        if typ & 0x80 != 0 {
            at += 2; // style id
            typ &= 0x7f;
        }
        // (n, values-in-file, increment-mode)
        let (n, count, incr) = match typ & 0x60 {
            0x40 => {
                let Some(n) = u16le(p, at) else { return };
                at += 2;
                (usize::from(n), usize::from(n), false)
            }
            0x60 => {
                let Some(n) = u16le(p, at) else { return };
                at += 2;
                (usize::from(n), 2, true)
            }
            0x20 => return,
            _ => (1, 1, false),
        };
        typ &= 0x1f;
        let mut values: Vec<Option<f64>> = Vec::with_capacity(count);
        let mut texts: Vec<Option<String>> = Vec::new();
        match typ {
            1 => {}
            2 | 3 => {
                for k in 0..count {
                    let Some(v) = u16le(p, at + 2 * k) else {
                        return;
                    };
                    values.push(Some(if typ == 2 {
                        f64::from(v)
                    } else {
                        f64::from(v as i16)
                    }));
                }
                at += 2 * count;
            }
            4 => {
                for k in 0..count {
                    let Some(b) = p.get(at + 4 * k..at + 4 * k + 4) else {
                        return;
                    };
                    values.push(double4([b[0], b[1], b[2], b[3]]));
                }
                at += 4 * count;
            }
            5 => {
                for k in 0..count {
                    let Some(v) = f64_at(p, at + 8 * k) else {
                        return;
                    };
                    values.push((!v.is_nan()).then_some(v));
                }
                at += 8 * count;
            }
            7 => {
                for k in 0..count {
                    let Some(ix) = u32le(p, at + 4 * k) else {
                        return;
                    };
                    values.push(Some(f64::from(ix)));
                }
                at += 4 * count;
            }
            8 => {
                for k in 0..count {
                    let Some(v) = f64_at(p, at + 14 * k) else {
                        return;
                    };
                    values.push((!v.is_nan()).then_some(v));
                }
                at += 14 * count;
            }
            _ => return,
        }
        if typ == 7 {
            // string indices (1-based); increments step the index
            for k in 0..n {
                let ix = if incr {
                    values[0].unwrap_or(0.0)
                        + k as f64 * values.get(1).copied().flatten().unwrap_or(0.0)
                } else {
                    values.get(k).copied().flatten().unwrap_or(0.0)
                };
                let ix = ix as usize;
                texts.push((ix >= 1).then(|| strings.get(ix - 1).cloned()).flatten());
            }
            for (k, t) in texts.into_iter().enumerate() {
                if let Some(t) = t {
                    insert(sheets, sheet, row + k, col, t);
                }
            }
        } else if typ != 1 {
            for k in 0..n {
                let v = if incr {
                    match (values[0], values.get(1).copied().flatten()) {
                        (Some(a), Some(step)) => Some(a + k as f64 * step),
                        (Some(a), None) => Some(a),
                        _ => None,
                    }
                } else {
                    values.get(k).copied().flatten()
                };
                if let Some(v) = v {
                    insert(sheets, sheet, row + k, col, number_text(v));
                }
            }
        }
        row += n;
    }
}

const CP437: [char; 128] = [
    '\u{00c7}', '\u{00fc}', '\u{00e9}', '\u{00e2}', '\u{00e4}', '\u{00e0}', '\u{00e5}', '\u{00e7}',
    '\u{00ea}', '\u{00eb}', '\u{00e8}', '\u{00ef}', '\u{00ee}', '\u{00ec}', '\u{00c4}', '\u{00c5}',
    '\u{00c9}', '\u{00e6}', '\u{00c6}', '\u{00f4}', '\u{00f6}', '\u{00f2}', '\u{00fb}', '\u{00f9}',
    '\u{00ff}', '\u{00d6}', '\u{00dc}', '\u{00a2}', '\u{00a3}', '\u{00a5}', '\u{20a7}', '\u{0192}',
    '\u{00e1}', '\u{00ed}', '\u{00f3}', '\u{00fa}', '\u{00f1}', '\u{00d1}', '\u{00aa}', '\u{00ba}',
    '\u{00bf}', '\u{2310}', '\u{00ac}', '\u{00bd}', '\u{00bc}', '\u{00a1}', '\u{00ab}', '\u{00bb}',
    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2502}', '\u{2524}', '\u{2561}', '\u{2562}', '\u{2556}',
    '\u{2555}', '\u{2563}', '\u{2551}', '\u{2557}', '\u{255d}', '\u{255c}', '\u{255b}', '\u{2510}',
    '\u{2514}', '\u{2534}', '\u{252c}', '\u{251c}', '\u{2500}', '\u{253c}', '\u{255e}', '\u{255f}',
    '\u{255a}', '\u{2554}', '\u{2569}', '\u{2566}', '\u{2560}', '\u{2550}', '\u{256c}', '\u{2567}',
    '\u{2568}', '\u{2564}', '\u{2565}', '\u{2559}', '\u{2558}', '\u{2552}', '\u{2553}', '\u{256b}',
    '\u{256a}', '\u{2518}', '\u{250c}', '\u{2588}', '\u{2584}', '\u{258c}', '\u{2590}', '\u{2580}',
    '\u{03b1}', '\u{00df}', '\u{0393}', '\u{03c0}', '\u{03a3}', '\u{03c3}', '\u{00b5}', '\u{03c4}',
    '\u{03a6}', '\u{0398}', '\u{03a9}', '\u{03b4}', '\u{221e}', '\u{03c6}', '\u{03b5}', '\u{2229}',
    '\u{2261}', '\u{00b1}', '\u{2265}', '\u{2264}', '\u{2320}', '\u{2321}', '\u{00f7}', '\u{2248}',
    '\u{00b0}', '\u{2219}', '\u{00b7}', '\u{221a}', '\u{207f}', '\u{00b2}', '\u{25a0}', '\u{00a0}',
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::InputFormat;

    fn convert(bytes: Vec<u8>) -> Result<DoclingDocument, ConversionError> {
        QuattroBackend.convert(&SourceDocument::from_bytes(
            "t.wq1",
            InputFormat::QuattroPro,
            bytes,
        ))
    }

    /// Markdown with table padding collapsed, so cell assertions do not
    /// depend on column widths.
    fn md(doc: &DoclingDocument) -> String {
        let mut out = String::new();
        let mut prev_space = false;
        for c in doc.export_to_markdown().chars() {
            if c == ' ' {
                if !prev_space {
                    out.push(' ');
                }
                prev_space = true;
            } else {
                out.push(c);
                prev_space = false;
            }
        }
        out
    }

    fn rec(op: u16, payload: &[u8]) -> Vec<u8> {
        let mut v = op.to_le_bytes().to_vec();
        v.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn wq1_cells_carry_a_format_byte_before_the_address() {
        let mut d = rec(0, &[0x20, 0x51]);
        // label at A1: fmt, col 0, sheet 0, row 0, align ', pascal "Item"
        d.extend(rec(
            0x0f,
            &[0x80, 0, 0, 0, 0, b'\'', 4, b'I', b't', b'e', b'm'],
        ));
        // integer 4001 at A3
        d.extend(rec(0x0d, &[0x80, 0, 0, 2, 0, 0xa1, 0x0f]));
        // number 2.5 at B3
        let mut p = vec![0x80, 1, 0, 2, 0];
        p.extend_from_slice(&2.5f64.to_le_bytes());
        d.extend(rec(0x0e, &p));
        // CP437 label at A2: 0x82 = é
        d.extend(rec(
            0x0f,
            &[0x80, 0, 0, 1, 0, b'\'', 4, b'c', b'a', b'f', 0x82],
        ));
        d.extend(rec(1, &[]));
        let doc = convert(d).unwrap();
        let md = md(&doc);
        assert!(md.contains("| Item"), "{md}");
        assert!(md.contains("caf\u{e9}"), "{md}");
        assert!(md.contains("| 4001 | 2.5 |"), "{md}");
    }

    #[test]
    fn wq2_cells_have_a_style_word_and_string_results() {
        let mut d = rec(0, &[0x21, 0x51]);
        // label at A1, no format byte: col, sheet, row, style, align, pascal
        d.extend(rec(
            0x0f,
            &[0, 0, 0, 0, 0x0f, 0xff, b'\'', 3, b'N', b'O', b':'],
        ));
        // formula at B1 with a NaN cache → filled by the 0x33 result "sal"
        let mut p = vec![1, 0, 0, 0, 0, 5];
        p.extend_from_slice(&f64::NAN.to_le_bytes());
        p.extend_from_slice(&[0x1f, 0, 0, 0]);
        d.extend(rec(0x10, &p));
        d.extend(rec(0x33, &[1, 0, 0, 0, 0, 6, 3, b's', b'a', b'l']));
        // integer 7 on sheet 1 at A1
        d.extend(rec(0x0d, &[0, 1, 0, 0, 0x0f, 0xff, 7, 0]));
        d.extend(rec(1, &[]));
        let doc = convert(d).unwrap();
        let md = md(&doc);
        assert!(md.contains("| NO: | sal |"), "{md}");
        assert!(md.contains("| 7 |"), "{md}");
    }

    #[test]
    fn wb1_records_mask_the_flag_bit_and_use_c_strings() {
        let mut d = rec(0, &[0x01, 0x10]);
        d.extend(rec(0x800f, &[0, 0, 0, 0, 1, 0, b'\'', b'X', 0, 0]));
        d.extend(rec(0x000d, &[0, 0, 1, 0, 2, 0, 10, 0]));
        d.extend(rec(1, &[]));
        let doc = convert(d).unwrap();
        let md = md(&doc);
        assert!(md.contains("| X |"), "{md}");
        assert!(md.contains("| 10 |"), "{md}");
    }

    #[test]
    fn double4_decodes_integers_and_floats() {
        // (first & 3) == 2: integer 5 → raw = (5 << 2) | 2
        assert_eq!(double4(((5u32 << 2) | 2).to_le_bytes()), Some(5.0));
        // negative: raw = ((-3 & 0x3fffffff) << 2) | 2
        let raw = (((-3i32) as u32 & 0x3fff_ffff) << 2) | 2;
        assert_eq!(double4(raw.to_le_bytes()), Some(-3.0));
        // exponent 0 with unit mantissa → zero; exponent 0x7ff → NaN marker
        assert_eq!(double4([0, 0, 0, 0]), Some(0.0));
        assert_eq!(double4([0, 0, 0xf0, 0x7f]), None);
        // 1.5: mantissa 1.5 = (0x10 + 8)/16, exponent 0x3ff → bytes
        // [0, 0, 0xf8, 0x3f]
        assert_eq!(double4([0, 0, 0xf8, 0x3f]), Some(1.5));
        // bit 0 divides by 100
        assert_eq!(double4([1, 0, 0xf8, 0x3f]), Some(0.015));
    }

    #[test]
    fn qpw_cells_resolve_strings_and_increments() {
        // zones: header, strings ["This", "That"], begin sheet 0, begin
        // column 0, cell run: row 0, string idx 1; then column 1 with an
        // increment run 10,20,30 of i16 and a list of two f64
        let mut d = Vec::new();
        let z = |id: u16, p: &[u8]| {
            let mut v = id.to_le_bytes().to_vec();
            v.extend_from_slice(&(p.len() as u16).to_le_bytes());
            v.extend_from_slice(p);
            v
        };
        let mut hdr = b"QPW9".to_vec();
        hdr.extend_from_slice(&[0, 0, 0, 0, 1, 0, 0, 0, 0, 0]);
        d.extend(z(1, &hdr));
        let mut strs = 2u32.to_le_bytes().to_vec();
        strs.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0]);
        for s in ["This", "That"] {
            strs.extend_from_slice(&(s.len() as u16).to_le_bytes());
            strs.push(0);
            strs.extend_from_slice(s.as_bytes());
        }
        d.extend(z(0x407, &strs));
        d.extend(z(0x601, &[0; 22]));
        d.extend(z(0xa01, &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        let mut cells = 0u32.to_le_bytes().to_vec();
        cells.extend_from_slice(&1u32.to_le_bytes());
        cells.extend_from_slice(&[0x07, 1, 0, 0, 0]);
        d.extend(z(0xc01, &cells));
        d.extend(z(0xa01, &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        let mut cells = 0u32.to_le_bytes().to_vec();
        cells.extend_from_slice(&5u32.to_le_bytes());
        // 0x60 | 3: increment run of 3 i16: first 10, step 10
        cells.extend_from_slice(&[0x63, 3, 0, 10, 0, 10, 0]);
        // 0x40 | 5: list of two f64
        cells.push(0x45);
        cells.extend_from_slice(&2u16.to_le_bytes());
        cells.extend_from_slice(&1.5f64.to_le_bytes());
        cells.extend_from_slice(&(-2.0f64).to_le_bytes());
        d.extend(z(0xc01, &cells));
        let sheets = read_qpw(&d);
        let g = &sheets[&0];
        assert_eq!(g[&(0, 0)], "This");
        assert_eq!(g[&(0, 1)], "10");
        assert_eq!(g[&(2, 1)], "30");
        assert_eq!(g[&(3, 1)], "1.5");
        assert_eq!(g[&(4, 1)], "-2");
    }

    #[test]
    fn corpus_samples_convert() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/quattro/sources");
        for (f, needle) in [
            ("formatcorpus_ksbase.wq1", "OBSERV"),
            (
                "formatcorpus_ks4000.wq2",
                "SATURATED HYDRAULIC CONDUCTIVITY",
            ),
            ("formatcorpus_test.wb1", "| X |"),
            ("formatcorpus_test.wb2", "| X |"),
            ("formatcorpus_test.wb3", "This is an example spreadsheet"),
            ("formatcorpus_test.qpw", "This is an example spreadsheet"),
        ] {
            let bytes = std::fs::read(dir.join(f)).unwrap();
            let doc = convert(bytes).unwrap_or_else(|e| panic!("{f}: {e}"));
            let md = md(&doc);
            assert!(md.contains(needle), "{f}:\n{md}");
        }
    }
}
