//! EBCDIC mainframe data backend (#252, docling#3926).
//!
//! An EBCDIC file carries no self-describing structure: the byte stream is a
//! sequence of fixed-width records whose fields only mean anything together
//! with the COBOL copybook that produced them. The copybook arrives as a JSON
//! layout (docling's `EbcdicLayout` schema, verbatim) — inline or as a file —
//! and each record schema becomes one table of the document, with the field
//! names as the header row and `skip` fields consumed but never emitted.
//!
//! Character data decodes through the cp037 family tables (cp500, cp1140 —
//! byte-for-byte the Python codecs of the same names); COBOL numerics unpack
//! from their nibbles: `packed_decimal` is COMP-3, `zoned_decimal` a signed
//! display numeric, `integer`/`unsigned_integer` big-endian COMP. A declared
//! scale renders exactly like Python's `Decimal(value).scaleb(-scale)` —
//! including the spec's scientific notation (`0E-7`), which live docling
//! output for scale-7 fields actually contains.
//!
//! docling.rs extensions over upstream: the layout may name its `encoding`
//! inline (upstream passes it as a separate backend option), and a source
//! converted from a path auto-discovers a `<stem>.layout.json` sidecar — the
//! corpus pairing — when no layout option is set.

use serde::Deserialize;

use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;
use docling_core::{DoclingDocument, Node, Table};

pub struct EbcdicBackend {
    /// The converter-level layout option: inline JSON (starts with `{`) or a
    /// filesystem path. `None` falls back to the sidecar.
    pub layout: Option<String>,
}

#[derive(Deserialize)]
struct Layout {
    records: Vec<RecordLayout>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    header_size: usize,
    #[serde(default)]
    footer_size: usize,
    #[serde(default)]
    record_length_field: Option<FieldDef>,
    #[serde(default)]
    record_type_field: Option<FieldDef>,
    /// docling.rs extension: upstream's `EbcdicBackendOptions.encoding`,
    /// carried in the layout so one JSON travels through every surface.
    #[serde(default = "default_encoding")]
    encoding: String,
}

fn default_encoding() -> String {
    "cp037".into()
}

#[derive(Deserialize)]
struct RecordLayout {
    fields: Vec<FieldDef>,
    #[serde(default = "default_record_name")]
    name: String,
    #[serde(default)]
    selector: Option<String>,
}

fn default_record_name() -> String {
    "record".into()
}

#[derive(Deserialize)]
struct FieldDef {
    name: String,
    size: usize,
    #[serde(default, rename = "type")]
    kind: FieldType,
    #[serde(default)]
    scale: u32,
}

#[derive(Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FieldType {
    #[default]
    String,
    Integer,
    UnsignedInteger,
    PackedDecimal,
    ZonedDecimal,
    Skip,
}

impl Layout {
    /// Mirrors upstream's pydantic validators, so a bad layout fails with the
    /// same complaints there and here.
    fn validate(&self) -> Result<(), String> {
        if self.records.is_empty() {
            return Err("a layout needs at least one record schema".into());
        }
        if self.records.iter().any(|r| r.fields.is_empty()) {
            return Err("every record schema needs at least one field".into());
        }
        if self
            .records
            .iter()
            .flat_map(|r| &r.fields)
            .chain(self.record_length_field.iter())
            .chain(self.record_type_field.iter())
            .any(|f| f.size == 0)
        {
            return Err("field sizes must be positive".into());
        }
        if self.records.len() > 1 && self.record_type_field.is_none() {
            return Err("record_type_field is required for a layout with several records".into());
        }
        if self.record_type_field.is_some() {
            let selectors: Vec<&Option<String>> =
                self.records.iter().map(|r| &r.selector).collect();
            if selectors.iter().any(|s| s.is_none()) {
                return Err("every record needs a selector when record_type_field is set".into());
            }
            let mut uniq: Vec<&str> = selectors.iter().filter_map(|s| s.as_deref()).collect();
            uniq.sort_unstable();
            uniq.dedup();
            if uniq.len() != self.records.len() {
                return Err("record selectors must be unique".into());
            }
        }
        Ok(())
    }

    fn prefix_size(&self) -> usize {
        self.record_length_field.as_ref().map_or(0, |f| f.size)
            + self.record_type_field.as_ref().map_or(0, |f| f.size)
    }

    fn decode_table(&self) -> Result<&'static [u16; 256], String> {
        match self.encoding.as_str() {
            "cp037" => Ok(&CP037),
            "cp500" => Ok(&CP500),
            "cp1140" => Ok(&CP1140),
            other => Err(format!(
                "unknown EBCDIC codec {other:?} (supported: cp037, cp500, cp1140)"
            )),
        }
    }
}

impl DeclarativeBackend for EbcdicBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let layout = self.resolve_layout(source)?;
        let layout: Layout = serde_json::from_str(&layout)
            .map_err(|e| ConversionError::Parse(format!("ebcdic layout: {e}")))?;
        layout
            .validate()
            .map_err(|e| ConversionError::Parse(format!("ebcdic layout: {e}")))?;
        let table = layout.decode_table().map_err(ConversionError::Parse)?;

        let mut doc = DoclingDocument::new(&source.name);
        if !layout.description.is_empty() {
            doc.push(Node::Paragraph {
                text: layout.description.clone(),
            });
        }

        // One row bucket per schema, keyed by position (schema order is the
        // emission order, like upstream's dict-of-lists).
        let mut rows: Vec<Vec<Vec<String>>> = layout.records.iter().map(|_| Vec::new()).collect();
        let data = &source.bytes;
        let end = data.len().saturating_sub(layout.footer_size);
        let mut offset = layout.header_size;
        while offset < end {
            // Record prefix: optional length, optional type selector.
            let mut length: Option<i128> = None;
            let mut record_type: Option<String> = None;
            if let Some(f) = &layout.record_length_field {
                let chunk = take(data, offset, f.size, end, &f.name)?;
                length = Some(decode_int_like(chunk, f, table)?);
                offset += f.size;
            }
            if let Some(f) = &layout.record_type_field {
                let chunk = take(data, offset, f.size, end, &f.name)?;
                record_type = Some(decode_value(chunk, f, table)?);
                offset += f.size;
            }
            let idx = layout
                .records
                .iter()
                .position(|r| match &layout.record_type_field {
                    None => true,
                    Some(_) => r.selector.as_deref() == record_type.as_deref(),
                })
                .ok_or_else(|| {
                    ConversionError::Parse(format!(
                        "ebcdic: no record layout matches record type {record_type:?}"
                    ))
                })?;
            let record = &layout.records[idx];
            let size = match length {
                None => record.fields.iter().map(|f| f.size).sum::<usize>(),
                Some(l) => {
                    let body = l - layout.prefix_size() as i128;
                    usize::try_from(body).map_err(|_| {
                        ConversionError::Parse(format!(
                            "ebcdic: record length {l} is shorter than the {}-byte record prefix",
                            layout.prefix_size()
                        ))
                    })?
                }
            };
            let body = take(data, offset, size, end, &record.name)?;
            let mut values = Vec::new();
            let mut field_off = 0usize;
            for f in &record.fields {
                let chunk = body.get(field_off..(field_off + f.size).min(body.len()));
                field_off += f.size;
                if f.kind == FieldType::Skip {
                    continue;
                }
                values.push(decode_value(chunk.unwrap_or(&[]), f, table)?);
            }
            rows[idx].push(values);
            offset += size;
        }

        let multi = layout.records.len() > 1;
        for (record, schema_rows) in layout.records.iter().zip(rows) {
            if schema_rows.is_empty() {
                continue;
            }
            if multi {
                // docling's `add_heading` default level renders as `##`.
                doc.push(Node::Heading {
                    level: 2,
                    text: record.name.clone(),
                });
            }
            let header: Vec<String> = record
                .fields
                .iter()
                .filter(|f| f.kind != FieldType::Skip)
                .map(|f| f.name.clone())
                .collect();
            let mut table_rows = vec![header];
            table_rows.extend(schema_rows);
            doc.push(Node::Table(Table {
                rows: table_rows,
                location: None,
                structure: None,
                cell_blocks: None,
                cells: None,
                caption: None,
            }));
        }
        Ok(doc)
    }
}

impl EbcdicBackend {
    /// The layout JSON text: the converter option (inline JSON or a path),
    /// else the `<stem>.layout.json` sidecar next to a path-loaded source.
    fn resolve_layout(&self, source: &SourceDocument) -> Result<String, ConversionError> {
        if let Some(opt) = &self.layout {
            if opt.trim_start().starts_with('{') {
                return Ok(opt.clone());
            }
            return std::fs::read_to_string(opt)
                .map_err(|e| ConversionError::Parse(format!("ebcdic layout {opt}: {e}")));
        }
        if let Some(path) = &source.path {
            let sidecar = path.with_extension("layout.json");
            if sidecar.exists() {
                return std::fs::read_to_string(&sidecar).map_err(|e| {
                    ConversionError::Parse(format!("ebcdic layout {}: {e}", sidecar.display()))
                });
            }
        }
        Err(ConversionError::Parse(
            "ebcdic: the format needs a copybook layout — pass one with the ebcdic_layout \
             option (inline JSON or a file path), or place a <name>.layout.json next to the \
             source file"
                .into(),
        ))
    }
}

fn take<'a>(
    data: &'a [u8],
    offset: usize,
    size: usize,
    end: usize,
    name: &str,
) -> Result<&'a [u8], ConversionError> {
    if offset + size > end {
        return Err(ConversionError::Parse(format!(
            "ebcdic: input ends inside {name:?}: {} of {size} bytes left",
            end.saturating_sub(offset)
        )));
    }
    Ok(&data[offset..offset + size])
}

/// Decode one field to its display string — the exact rendering Python's
/// `str()` gives docling's decoded values.
fn decode_value(data: &[u8], f: &FieldDef, table: &[u16; 256]) -> Result<String, ConversionError> {
    match f.kind {
        FieldType::String => {
            // Decode, drop C0/C1 control characters (EBCDIC padding and
            // delimiters land there), trim whitespace — upstream's `_string`.
            let s: String = data
                .iter()
                .map(|&b| decode_char(table, b))
                .filter(|&c| !(c as u32 <= 0x1F || (0x7F..=0x9F).contains(&(c as u32))))
                .collect();
            Ok(s.trim().to_string())
        }
        FieldType::Integer | FieldType::UnsignedInteger => {
            let v = decode_int_like(data, f, table)?;
            Ok(scale_decimal(&v.unsigned_abs().to_string(), v < 0, f.scale))
        }
        FieldType::PackedDecimal => {
            // COMP-3: two digits per byte, sign in the trailing nibble.
            let mut digits = String::new();
            for (i, &b) in data.iter().enumerate() {
                digits.push(char::from(b'0' + (b >> 4)));
                if i + 1 < data.len() {
                    digits.push(char::from(b'0' + (b & 0x0F)));
                }
            }
            let sign_nibble = data.last().map_or(0x0C, |b| b & 0x0F);
            if digits.bytes().any(|b| !b.is_ascii_digit()) {
                return Err(decode_err(f, data));
            }
            Ok(scale_decimal(
                &digits,
                matches!(sign_nibble, 0x0B | 0x0D),
                f.scale,
            ))
        }
        FieldType::ZonedDecimal => {
            // Signed display numeric: one digit per low nibble, sign in the
            // last byte's zone nibble.
            let mut digits = String::new();
            for &b in data {
                if b & 0x0F > 9 {
                    return Err(decode_err(f, data));
                }
                digits.push(char::from(b'0' + (b & 0x0F)));
            }
            let negative = data.last().is_some_and(|b| matches!(b >> 4, 0x0B | 0x0D));
            Ok(scale_decimal(&digits, negative, f.scale))
        }
        FieldType::Skip => Ok(String::new()),
    }
}

/// Big-endian binary integers, used both for value fields and the record
/// length prefix. Capped at 16 bytes (i128) — Python is unbounded, but no
/// real copybook carries a wider COMP field.
fn decode_int_like(data: &[u8], f: &FieldDef, table: &[u16; 256]) -> Result<i128, ConversionError> {
    match f.kind {
        FieldType::Integer | FieldType::UnsignedInteger => {
            if data.is_empty() || data.len() > 16 {
                return Err(decode_err(f, data));
            }
            let negative = f.kind == FieldType::Integer && data[0] & 0x80 != 0;
            let mut v: i128 = if negative { -1 } else { 0 };
            for &b in data {
                v = (v << 8) | i128::from(b);
            }
            Ok(v)
        }
        // A length prefix may also be declared as a display numeric.
        _ => decode_value(data, f, table)?
            .parse::<i128>()
            .map_err(|_| decode_err(f, data)),
    }
}

fn decode_err(f: &FieldDef, data: &[u8]) -> ConversionError {
    let hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
    ConversionError::Parse(format!(
        "ebcdic: cannot decode field {:?} from '{hex}'",
        f.name
    ))
}

fn decode_char(table: &[u16; 256], b: u8) -> char {
    char::from_u32(u32::from(table[b as usize])).unwrap_or('\u{FFFD}')
}

/// Render an unsigned digit string + sign at a decimal scale exactly like
/// `str(Decimal(value).scaleb(-scale))`: plain notation while the adjusted
/// exponent stays ≥ -6, the spec's scientific notation beyond (`0E-7` — which
/// scale-7 copybook fields really produce). Trailing zeros are kept: a zero
/// at scale 4 is `0.0000`, matching docling's committed output.
fn scale_decimal(raw_digits: &str, negative: bool, scale: u32) -> String {
    let digits = raw_digits.trim_start_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let sign = if negative && digits != "0" { "-" } else { "" };
    // `Decimal(-0)` is 0; but a negative zero *scaled* keeps its sign in
    // Python only via Decimal("-0") construction — int(-0) == 0, so no sign.
    if scale == 0 {
        return format!("{sign}{digits}");
    }
    let adjusted = digits.len() as i64 - 1 - i64::from(scale);
    if adjusted >= -6 {
        if digits.len() > scale as usize {
            let (int_part, frac) = digits.split_at(digits.len() - scale as usize);
            format!("{sign}{int_part}.{frac}")
        } else {
            let frac = format!("{digits:0>width$}", width = scale as usize);
            format!("{sign}0.{frac}")
        }
    } else {
        // to-scientific-string: one digit, optional fraction, E<adjusted>.
        let (head, rest) = digits.split_at(1);
        if rest.is_empty() {
            format!("{sign}{head}E{adjusted}")
        } else {
            format!("{sign}{head}.{rest}E{adjusted}")
        }
    }
}

include!("ebcdic_tables.rs");

#[cfg(test)]
mod tests {
    use super::scale_decimal;

    #[test]
    fn decimal_rendering_matches_python() {
        // str(Decimal(v).scaleb(-scale)) reference values.
        assert_eq!(scale_decimal("0", false, 0), "0");
        assert_eq!(scale_decimal("0", false, 4), "0.0000");
        assert_eq!(scale_decimal("12345", false, 2), "123.45");
        assert_eq!(scale_decimal("12345", true, 2), "-123.45");
        assert_eq!(scale_decimal("5", false, 4), "0.0005");
        assert_eq!(scale_decimal("0", false, 7), "0E-7");
        assert_eq!(scale_decimal("5", true, 7), "-5E-7");
        assert_eq!(scale_decimal("52", false, 7), "0.0000052");
        assert_eq!(scale_decimal("007", false, 1), "0.7");
    }
}
