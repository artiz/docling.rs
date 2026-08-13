#!/usr/bin/env python3
"""Synthesize Lotus-family spreadsheet fixtures for docling.rs (#216).

Record layouts follow Gnumeric's lotus-123 importer (see lotus.c);
LibreOffice's import filters serve as the independent read oracle.
"""
import struct, sys, os

OUT = sys.argv[1] if len(sys.argv) > 1 else "."

def rec(op, payload):
    return struct.pack("<HH", op, len(payload)) + payload

# ---- old family (WK1/WKS/Symphony/Quattro DOS): [fmt u8][col u16][row u16]
FMT = 0xFF  # default format byte

def o_int(col, row, v):
    return rec(0x0D, struct.pack("<BHHh", FMT, col, row, v))

def o_num(col, row, v):
    return rec(0x0E, struct.pack("<BHHd", FMT, col, row, v))

def o_label(col, row, s, prefix=b"'"):
    return rec(0x0F, struct.pack("<BHH", FMT, col, row) + prefix + s.encode("cp1252") + b"\0")

def o_formula_num(col, row, v, code=b"\x00"):
    # cached f64 result + dummy bytecode
    return rec(0x10, struct.pack("<BHHdH", FMT, col, row, v, len(code)) + code)

def o_formula_str(col, row, s, code=b"\x00"):
    # NaN-boxed cache (0x7ff0 pattern at bytes 11-12) + following STRING record
    cache = b"\x00" * 6 + struct.pack("<H", 0x7FF0)
    f = rec(0x10, struct.pack("<BHH", FMT, col, row) + cache + struct.pack("<H", len(code)) + code)
    s_rec = rec(0x33, struct.pack("<BHH", FMT, col, row) + s.encode("cp1252") + b"\0")
    return f + s_rec

def old_file(version, body):
    return rec(0x00, struct.pack("<H", version)) + body + rec(0x01, b"")

# Ducks data at the same coordinates as odf_table_with_title_01.slk
# (SYLK is 1-based; these records are 0-based -> col/row minus one).
def ducks_body(int_cell, num_cell, label_cell, formula_cell):
    b = label_cell(1, 1, "Number of freshwater ducks per year")
    b += label_cell(1, 3, "Year") + label_cell(2, 3, "Freshwater Ducks")
    years = [2019, 2020, 2021, 2022, 2023, 2024]
    ducks = [120, 135, 150, 170, 160, 180]
    for i, (y, d) in enumerate(zip(years, ducks)):
        row = 4 + i
        b += int_cell(1, row, y)
        # exercise every numeric record kind; all render as integers
        if i % 3 == 0:
            b += int_cell(2, row, d)
        elif i % 3 == 1:
            b += num_cell(2, row, float(d))
        else:
            b += formula_cell(2, row, float(d))
    return b

with open(os.path.join(OUT, "ducks.wk1"), "wb") as f:
    f.write(old_file(0x0406, ducks_body(o_int, o_num, o_label, o_formula_num)))

with open(os.path.join(OUT, "ducks_123r1.wks"), "wb") as f:
    f.write(old_file(0x0404, ducks_body(o_int, o_num, o_label, o_formula_num)))

# A string-formula cell exercised separately (LibreOffice renders formula
# cells it cannot parse differently; keep the oracle corpus clean).
with open(os.path.join(OUT, "strfml.wk1"), "wb") as f:
    body = o_label(0, 0, "kind") + o_label(1, 0, "value")
    body += o_label(0, 1, "join") + o_formula_str(1, 1, "a-b")
    body += o_label(0, 2, "num") + o_formula_num(1, 2, 6.5)
    f.write(old_file(0x0406, body))

# ---- new family (WK3): [row u16][sheet u8][col u8]
def n_label(sheet, row, col, s, prefix=b"'"):
    return rec(0x16, struct.pack("<HBB", row, sheet, col) + prefix + s.encode("cp1252") + b"\0")

def treal(v):
    # 10-byte extended float: u64 mantissa, u16 sign|exp (bias 16383, mant<<63)
    if v == 0:
        return struct.pack("<QH", 0, 0)
    sign = 0x8000 if v < 0 else 0
    v = abs(v)
    m, e = 0.0, 0
    import math
    m, e = math.frexp(v)          # v = m * 2^e, m in [0.5, 1)
    mant = int(m * (1 << 64))     # top-bit-set 64-bit mantissa
    exp = e - 1 + 16383           # extended-float exponent for mant*2^(exp-63)...
    # value = mant * 2^(exp-16383-63) = m*2^64 * 2^(e-1-63) = m*2^e  ✓
    return struct.pack("<QH", mant, sign | exp)

def n_extfloat(sheet, row, col, v):
    return rec(0x17, struct.pack("<HBB", row, sheet, col) + treal(v))

def n_smallnum_int(sheet, row, col, v):
    return rec(0x18, struct.pack("<HBBh", row, sheet, col, v << 1))

def n_smallnum_frac(sheet, row, col, mant, fidx):
    # odd encoding: factor table index in bits 1-3, mantissa from bit 4
    return rec(0x18, struct.pack("<HBBh", row, sheet, col, (mant << 4) | (fidx << 1) | 1))

def n_packed(sheet, row, col, mag, exp, neg=False, div=False):
    u = (mag << 6) | (0x20 if neg else 0) | (0x10 if div else 0) | exp
    return rec(0x25, struct.pack("<HBBI", row, sheet, col, u))

def n_number2(sheet, row, col, v):
    return rec(0x27, struct.pack("<HBBd", row, sheet, col, v))

def n_na(sheet, row, col):
    return rec(0x15, struct.pack("<HBB", row, sheet, col))

def n_formula3(sheet, row, col, v, code=b"\x00"):
    return rec(0x19, struct.pack("<HBB", row, sheet, col) + treal(v) + code)

def n_formulastring(sheet, row, col, s):
    # cache says "string pending" (ff ff at 8-9, 0xe0 at 7)
    pend = b"\x00" * 7 + b"\xe0\xff\xff"
    f = rec(0x19, struct.pack("<HBB", row, sheet, col) + pend + b"\x00")
    return f + rec(0x1A, struct.pack("<HBB", row, sheet, col) + s.encode("cp1252") + b"\0")

wk3 = rec(0x00, struct.pack("<H", 0x1000) + b"\x00" * 24)  # BOF, 26 bytes
# sheet 0: mixed numeric record kinds
wk3 += n_label(0, 0, 0, "City") + n_label(0, 0, 1, "Total") + n_label(0, 0, 2, "Share")
wk3 += n_label(0, 1, 0, "Aachen") + n_extfloat(0, 1, 1, 1200.0) + n_extfloat(0, 1, 2, 0.25)
wk3 += n_label(0, 2, 0, "Bremen") + n_smallnum_int(0, 2, 1, 950) + n_smallnum_frac(0, 2, 2, 18, 3)  # 18/200 = 0.09
wk3 += n_label(0, 3, 0, "Celle") + n_packed(0, 3, 1, 71, 1) + n_packed(0, 3, 2, 16, 2, div=True)   # 710, 0.16
wk3 += n_label(0, 4, 0, "Dresden") + n_number2(0, 4, 1, 830.0) + n_formula3(0, 4, 2, 0.5)
# sheet 1: labels, a string formula, an N/A
wk3 += n_label(1, 0, 0, "Quarter") + n_label(1, 0, 1, "Code")
wk3 += n_label(1, 1, 0, "Q1") + n_formulastring(1, 1, 1, "A-17")
wk3 += n_label(1, 2, 0, "Q2") + n_na(1, 2, 1)
wk3 += rec(0x01, b"")
with open(os.path.join(OUT, "cities.wk3"), "wb") as f:
    f.write(wk3)

# ---- MS Works v3: [col u16][row u16][fmt u16]
def w_num(col, row, v):
    return rec(0x0E, struct.pack("<HHHd", col, row, 0, v))

def w_label(col, row, s):
    return rec(0x0F, struct.pack("<HHH", col, row, 0) + s.encode("cp1252") + b"\0")

def w_formula_num(col, row, v, code=b"\x00"):
    return rec(0x10, struct.pack("<HHHdH", col, row, 0, v, len(code)) + code)

def w_small_float(col, row, v, div100=False):
    # inverse of gnumeric's unpack: bits = f32 bits; raw = (bits & 0xfc000000) | ((bits >> 3) & 0x3fffffe) | flag
    bits = struct.unpack("<I", struct.pack("<f", v))[0]
    raw = (bits & 0xFC000000) | ((bits >> 3) & 0x03FFFFFE) | (1 if div100 else 0)
    return rec(0x545B, struct.pack("<BBH", col, 0, row) + struct.pack("<H", 0) + struct.pack("<I", raw))

wks = rec(0xFF, struct.pack("<H", 0x0404))
wks += w_label(0, 0, "Item") + w_label(1, 0, "Qty") + w_label(2, 0, "Price")
wks += w_label(0, 1, "Bolt") + w_num(1, 1, 40.0) + w_small_float(2, 1, 275.0, div100=True)  # 2.75
wks += w_label(0, 2, "Nut") + w_formula_num(1, 2, 60.0) + w_small_float(2, 2, 1.5)
wks += rec(0x01, b"")
with open(os.path.join(OUT, "works_v3.wks"), "wb") as f:
    f.write(wks)

print("generated")
