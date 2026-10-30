//! What the PDF reader had to repair to read a file (#934): typed provenance
//! for callers that must tell recovered input from well-formed input — an
//! admission guard refusing repaired sources, say.
//!
//! The reader repairs silently, by design (degradation over failure): lopdf
//! 0.45 recovers a stream whose `/Length` does not end at `endstream` by
//! searching for the unambiguous `endstream … endobj` boundary, and then
//! `Stream::new` overwrites `/Length` with the recovered size — the declared
//! value and the fact of the repair do not survive in the loaded object, only
//! in a `log::warn!`. [`crate::textparse`]'s own repairs (19-byte xref entries
//! padded to 20, overstated lengths rewritten in place) leave no trace either.
//! So this reads the *original* bytes once more, on demand, with lopdf's own
//! stream grammar — `dict space "stream" space0 eol`, then exactly `/Length`
//! bytes, an optional EOL and `endstream` — and reports every stream that
//! fails it, with the declared and the recovered length. A conversion runs
//! it only when asked (`pdf_report_repairs`, off by default), and the
//! document it produces is the same either way.

use std::collections::HashMap;

pub use docling_core::PdfRepair;

/// Every repair reading `bytes` takes (#934), in file order; empty for a
/// well-formed file. The input as given — encrypted files included, whose
/// lengths are those of the encrypted bytes. Cost: one linear pass over the
/// file and one lopdf load (a second when the xref needs its repair).
pub fn pdf_repairs(bytes: &[u8]) -> Vec<PdfRepair> {
    let mut repairs = Vec::new();
    // The one load: a file lopdf reads as it is, or — the reader's xref
    // repair — once its 19-byte entries are padded.
    let doc = lopdf::Document::load_mem(bytes).ok().or_else(|| {
        let fixed = crate::textparse::pad_short_xref_entries(bytes).ok()?;
        let doc = lopdf::Document::load_mem(&fixed).ok()?;
        repairs.push(PdfRepair::XrefEntriesPadded);
        Some(doc)
    });
    let objects = scan_objects(bytes);
    let integers: HashMap<(u32, u16), i64> = objects
        .iter()
        .filter_map(|o| Some((o.id, o.integer?)))
        .collect();
    // An indirect length can live in an object stream (PDF 1.5+), out of
    // the raw scan's reach: ask the loaded document — integers are not
    // touched by any repair.
    let resolve = |id: (u32, u16)| -> Option<i64> {
        if let Some(&n) = integers.get(&id) {
            return Some(n);
        }
        doc.as_ref()?.get_object(id).ok()?.as_i64().ok()
    };
    for o in &objects {
        let Some(stream) = &o.stream else { continue };
        let declared = match stream.length {
            Some(LengthValue::Direct(n)) => Some(n),
            Some(LengthValue::Ref(id)) => resolve(id),
            None => None,
        }
        .filter(|&n| n >= 0);
        let ends_at_endstream = declared
            .and_then(|n| usize::try_from(n).ok())
            .and_then(|n| stream.data.checked_add(n))
            .is_some_and(|end| ends_at_endstream(bytes, end));
        if !ends_at_endstream {
            repairs.push(PdfRepair::StreamLength {
                object: o.id,
                offset: o.offset,
                declared,
                recovered: recover_length(bytes, stream.data),
            });
        }
    }
    repairs
}

/// lopdf's acceptance test: `take(length)`, an optional EOL, `endstream`.
fn ends_at_endstream(bytes: &[u8], end: usize) -> bool {
    let Some(rest) = bytes.get(end..) else {
        return false;
    };
    let rest = rest
        .strip_prefix(b"\r\n")
        .or_else(|| rest.strip_prefix(b"\n"))
        .or_else(|| rest.strip_prefix(b"\r"))
        .unwrap_or(rest);
    rest.starts_with(b"endstream")
}

/// lopdf's `recover_stream_length`: the data runs to the EOL before the one
/// `endstream` that is followed by whitespace and `endobj`; none, or two
/// candidates before the next object, recover nothing (lopdf bounds its
/// search by the next object's xref offset; the next `N G obj` header here).
fn recover_length(bytes: &[u8], data: usize) -> Option<usize> {
    let window = &bytes[data..];
    let (first_end, after_first) = boundary(window, 0)?;
    let bound = next_object_header(window, after_first).unwrap_or(window.len());
    match boundary(&window[..bound], after_first) {
        Some(_) => None,
        None => Some(first_end),
    }
}

/// The first `EOL endstream space endobj` in `window` from `from`: the data
/// end before it and the offset after its `endstream`.
fn boundary(window: &[u8], from: usize) -> Option<(usize, usize)> {
    const ENDSTREAM: &[u8] = b"endstream";
    let mut at = from;
    while let Some(rel) = find(&window[at..], ENDSTREAM) {
        let pos = at + rel;
        at = pos + ENDSTREAM.len();
        let before = &window[..pos];
        let data_end = if before.ends_with(b"\r\n") {
            pos - 2
        } else if before.ends_with(b"\n") || before.ends_with(b"\r") {
            pos - 1
        } else {
            continue;
        };
        let after = &window[at..];
        let ws = after.iter().take_while(|&&c| is_whitespace(c)).count();
        if !after[ws..].starts_with(b"endobj")
            || after.get(ws + 6).is_some_and(|&c| !is_whitespace(c))
        {
            continue;
        }
        return Some((data_end, at));
    }
    None
}

/// The offset of the next `N G obj` header in `window` from `from`.
fn next_object_header(window: &[u8], from: usize) -> Option<usize> {
    let mut at = from;
    while let Some(rel) = find(&window[at..], b"obj") {
        let kw = at + rel;
        at = kw + 3;
        if header_before(window, kw).is_some() {
            return Some(kw);
        }
    }
    None
}

/// A stream's declared length as written.
enum LengthValue {
    Direct(i64),
    Ref((u32, u16)),
}

struct StreamInfo {
    /// `/Length` in the dictionary, if any.
    length: Option<LengthValue>,
    /// Offset of the first data byte (after `stream` and its EOL).
    data: usize,
}

struct RawObject {
    id: (u32, u16),
    offset: usize,
    /// The object's value when it is a bare integer — an indirect length.
    integer: Option<i64>,
    stream: Option<StreamInfo>,
}

/// Every `N G obj` in the file, front to back, skipping stream data so that
/// binary content is never read as syntax. Objects inside object streams
/// cannot be streams (ISO 32000 7.5.7), so the top level is all there is.
fn scan_objects(bytes: &[u8]) -> Vec<RawObject> {
    let mut objects = Vec::new();
    let mut at = 0;
    while let Some(rel) = find(&bytes[at..], b"obj") {
        let kw = at + rel;
        at = kw + 3;
        // `obj` as a keyword: not `endobj`, followed by a delimiter.
        if bytes
            .get(kw + 3)
            .is_some_and(|&c| !is_whitespace(c) && !is_delimiter(c))
        {
            continue;
        }
        let Some((id, offset)) = header_before(bytes, kw) else {
            continue;
        };
        let body = skip_space(bytes, kw + 3);
        let mut object = RawObject {
            id,
            offset,
            integer: None,
            stream: None,
        };
        if bytes[body..].starts_with(b"<<") {
            let Some((dict_end, length)) = dictionary(bytes, body) else {
                objects.push(object);
                continue;
            };
            let after = skip_space(bytes, dict_end);
            if bytes[after..].starts_with(b"stream") {
                let mut data = after + 6;
                while matches!(bytes.get(data), Some(b' ' | b'\t')) {
                    data += 1;
                }
                if bytes[data..].starts_with(b"\r\n") {
                    data += 2;
                } else if matches!(bytes.get(data), Some(b'\n' | b'\r')) {
                    data += 1;
                } else {
                    // No EOL after `stream`: not a stream lopdf reads either.
                    objects.push(object);
                    at = after + 6;
                    continue;
                }
                // Resume past the data: at the declared end when it holds
                // (direct lengths only — an indirect one is resolved later),
                // else at the `endstream` that follows.
                at = match &length {
                    Some(LengthValue::Direct(n))
                        if usize::try_from(*n)
                            .ok()
                            .and_then(|n| data.checked_add(n))
                            .is_some_and(|end| ends_at_endstream(bytes, end)) =>
                    {
                        data + *n as usize
                    }
                    _ => find(&bytes[data..], b"endstream").map_or(bytes.len(), |r| data + r),
                };
                object.stream = Some(StreamInfo { length, data });
            } else {
                at = dict_end;
            }
        } else {
            let (value, end) = integer(bytes, body);
            if let Some(v) = value {
                if bytes[skip_space(bytes, end)..].starts_with(b"endobj") {
                    object.integer = Some(v);
                }
            }
        }
        objects.push(object);
    }
    objects
}

/// `N G` immediately before the `obj` keyword at `kw`, at a token boundary:
/// the object id and the offset of `N`.
fn header_before(bytes: &[u8], kw: usize) -> Option<((u32, u16), usize)> {
    let mut i = kw;
    let ws_end = i;
    while i > 0 && is_whitespace(bytes[i - 1]) {
        i -= 1;
    }
    if i == ws_end {
        return None;
    }
    let gen_end = i;
    while i > 0 && bytes[i - 1].is_ascii_digit() {
        i -= 1;
    }
    let gen = std::str::from_utf8(&bytes[i..gen_end])
        .ok()?
        .parse::<u16>()
        .ok()?;
    let sp = i;
    while i > 0 && is_whitespace(bytes[i - 1]) {
        i -= 1;
    }
    if i == sp {
        return None;
    }
    let num_end = i;
    while i > 0 && bytes[i - 1].is_ascii_digit() {
        i -= 1;
    }
    let num = std::str::from_utf8(&bytes[i..num_end])
        .ok()?
        .parse::<u32>()
        .ok()?;
    if i > 0 && !is_whitespace(bytes[i - 1]) && !is_delimiter(bytes[i - 1]) {
        return None;
    }
    Some(((num, gen), i))
}

/// The dictionary opening at `start` (`<<`): the offset just past its `>>`
/// and its top-level `/Length` value. Strings, hex strings, comments and
/// nested dictionaries are skipped, so `>>` or `/Length` inside them do not
/// count; `/Length1`–`/Length3` (font programs) are other keys.
fn dictionary(bytes: &[u8], start: usize) -> Option<(usize, Option<LengthValue>)> {
    let mut depth = 0usize;
    let mut i = start;
    let mut length = None;
    while i < bytes.len() {
        match bytes[i] {
            b'<' if bytes.get(i + 1) == Some(&b'<') => {
                depth += 1;
                i += 2;
            }
            b'>' if bytes.get(i + 1) == Some(&b'>') => {
                depth -= 1;
                i += 2;
                if depth == 0 {
                    return Some((i, length));
                }
            }
            b'<' => {
                i = bytes[i..]
                    .iter()
                    .position(|&c| c == b'>')
                    .map(|p| i + p + 1)?
            }
            b'(' => i = literal_string_end(bytes, i)?,
            b'%' => {
                while i < bytes.len() && !matches!(bytes[i], b'\r' | b'\n') {
                    i += 1;
                }
            }
            b'/' => {
                let name_end = i
                    + 1
                    + bytes[i + 1..]
                        .iter()
                        .take_while(|&&c| !is_whitespace(c) && !is_delimiter(c))
                        .count();
                if depth == 1 && &bytes[i + 1..name_end] == b"Length" {
                    length = length_value(bytes, name_end);
                }
                i = name_end;
            }
            _ => i += 1,
        }
    }
    None
}

/// `/Length`'s value at `at`: an integer, or `N G R`.
fn length_value(bytes: &[u8], at: usize) -> Option<LengthValue> {
    let (first, end) = integer(bytes, skip_space(bytes, at));
    let first = first?;
    let (gen, gen_end) = integer(bytes, skip_space(bytes, end));
    if let Some(gen) = gen {
        let r = skip_space(bytes, gen_end);
        if bytes.get(r) == Some(&b'R')
            && bytes
                .get(r + 1)
                .is_none_or(|&c| is_whitespace(c) || is_delimiter(c))
        {
            return Some(LengthValue::Ref((
                u32::try_from(first).ok()?,
                u16::try_from(gen).ok()?,
            )));
        }
    }
    Some(LengthValue::Direct(first))
}

/// A signed integer token at `at` (not a real: `3.0` is not an integer), and
/// the offset after it.
fn integer(bytes: &[u8], at: usize) -> (Option<i64>, usize) {
    let mut i = at;
    if matches!(bytes.get(i), Some(b'+' | b'-')) {
        i += 1;
    }
    let digits = bytes[i.min(bytes.len())..]
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .count();
    let end = i + digits;
    if digits == 0 || bytes.get(end) == Some(&b'.') {
        return (None, at);
    }
    let value = std::str::from_utf8(&bytes[at..end])
        .ok()
        .and_then(|s| s.parse().ok());
    (value, end)
}

/// Past the `)` closing the literal string opening at `start`: balanced
/// parentheses, backslash escapes.
fn literal_string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Past whitespace and comments (lopdf's `space`).
fn skip_space(bytes: &[u8], mut i: usize) -> usize {
    loop {
        while i < bytes.len() && is_whitespace(bytes[i]) {
            i += 1;
        }
        if bytes.get(i) == Some(&b'%') {
            while i < bytes.len() && !matches!(bytes[i], b'\r' | b'\n') {
                i += 1;
            }
        } else {
            return i;
        }
    }
}

fn is_whitespace(c: u8) -> bool {
    b" \t\n\r\0\x0C".contains(&c)
}

fn is_delimiter(c: u8) -> bool {
    b"()<>[]{}/%".contains(&c)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #934's reproduction: a one-page PDF whose content stream `q Q\n` (4
    /// bytes up to `endstream`) declares `declared` bytes, and `objects`
    /// spliced in after it.
    fn pdf(length: &str, extra: &[&str]) -> Vec<u8> {
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".to_owned(),
            format!("<< /Length {length} >>\nstream\nq Q\nendstream"),
        ];
        objects.extend(extra.iter().map(|s| s.to_string()));
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0];
        for (i, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
        }
        let xref = bytes.len();
        let n = objects.len() + 1;
        bytes.extend_from_slice(format!("xref\n0 {n}\n0000000000 65535 f \n").as_bytes());
        for offset in &offsets[1..] {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!("trailer\n<< /Size {n} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        bytes
    }

    #[test]
    fn a_well_formed_file_reports_nothing() {
        // 4 = `q Q\n` (the EOL before `endstream` counted), 3 = `q Q`: lopdf
        // accepts both — the EOL after the data is optional.
        assert_eq!(pdf_repairs(&pdf("4", &[])), []);
        assert_eq!(pdf_repairs(&pdf("3", &[])), []);
    }

    #[test]
    fn an_overstated_length_is_reported_with_both_lengths() {
        let bytes = pdf("5", &[]);
        let offset = find(&bytes, b"4 0 obj").unwrap();
        assert_eq!(
            pdf_repairs(&bytes),
            [PdfRepair::StreamLength {
                object: (4, 0),
                offset,
                declared: Some(5),
                recovered: Some(3),
            }]
        );
        // What lopdf loads — and why the loaded object cannot tell: the
        // recovered content, `/Length` overwritten to match it.
        let doc = lopdf::Document::load_mem(&bytes).unwrap();
        let stream = doc.get_object((4, 0)).unwrap().as_stream().unwrap();
        assert_eq!(stream.content, b"q Q");
        assert_eq!(stream.dict.get(b"Length").unwrap().as_i64().unwrap(), 3);
    }

    #[test]
    fn understated_missing_and_indirect_lengths() {
        let repair = |length: &str, extra: &[&str]| match &pdf_repairs(&pdf(length, extra))[..] {
            [PdfRepair::StreamLength {
                declared,
                recovered,
                ..
            }] => Some((*declared, *recovered)),
            [] => None,
            other => panic!("{other:?}"),
        };
        assert_eq!(repair("2", &[]), Some((Some(2), Some(3))));
        assert_eq!(repair("-1", &[]), Some((None, Some(3))));
        assert_eq!(repair("/Foo", &[]), Some((None, Some(3))));
        // `/Length 5 0 R` → 4: well-formed; → 9: overstated.
        assert_eq!(repair("5 0 R", &["4"]), None);
        assert_eq!(repair("5 0 R", &["9"]), Some((Some(9), Some(3))));
        // A reference to nothing.
        assert_eq!(repair("7 0 R", &["4"]), Some((None, Some(3))));
    }

    #[test]
    fn short_xref_entries_lopdf_reads_are_no_repair() {
        // 19-byte entries (a bare LF): lopdf 0.45 reads them as they are, so
        // the reader's padding fallback does not fire and is not reported;
        // the overstated stream still is.
        let bytes = pdf("5", &[]);
        let text = String::from_utf8(bytes).unwrap();
        let short = text.replace(" n \n", " n\n").replace(" f \n", " f\n");
        assert!(lopdf::Document::load_mem(short.as_bytes()).is_ok());
        assert!(matches!(
            &pdf_repairs(short.as_bytes())[..],
            [PdfRepair::StreamLength {
                object: (4, 0),
                declared: Some(5),
                recovered: Some(3),
                ..
            }]
        ));
    }

    #[test]
    fn binary_data_and_other_length_keys_are_not_syntax() {
        // A font program's `/Length1` is another key; data holding `obj`,
        // `>>` and `endstream`-free binary is skipped by its declared length.
        let font = "<< /Length1 99 /Length 12 >>\nstream\n1 0 obj >> (\nendstream";
        assert_eq!(pdf_repairs(&pdf("4", &[font])), []);
    }

    #[test]
    fn a_stream_without_an_unambiguous_end_recovers_nothing() {
        // Two `endstream … endobj` candidates inside one object's span: lopdf
        // refuses to guess and drops the stream.
        let bytes = pdf("5", &[]);
        let at = find(&bytes, b"q Q\nendstream").unwrap() + 4;
        let mut twice = bytes[..at].to_vec();
        twice.extend_from_slice(b"endstream\nendobj\nQ\n");
        twice.extend_from_slice(&bytes[at..]);
        assert!(matches!(
            &pdf_repairs(&twice)[..],
            [PdfRepair::StreamLength {
                recovered: None,
                ..
            }]
        ));
    }
}
