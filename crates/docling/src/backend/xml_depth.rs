//! Element-nesting guard for XML inputs.
//!
//! `roxmltree`'s tokenizer recurses once per nested element
//! (`parse_element` → `parse_content` → `parse_element` …), so a document
//! nested a couple of thousand levels deep overflows a 2 MiB worker-thread
//! stack before any backend code runs — an abort that takes the whole process
//! (and a docling-serve instance) with it. A 35 KB `.docx` with 2 000 tables
//! nested one inside the next did exactly that. Every XML part and document
//! goes through [`check`] first: an iterative scan of the markup that counts
//! open elements and rejects anything nested past [`max_depth`] (default
//! 512, `DOCLING_RS_MAX_XML_DEPTH`), long before the stack is at risk and far
//! beyond what any real document uses (OOXML and ODF stay under a few dozen).
//!
//! The scanner is a limit check, not a parser: it walks the bytes once,
//! skipping comments, CDATA sections, processing instructions and the
//! DOCTYPE (including an internal subset) whole, and quoted attribute values
//! inside tags, so a `<` in any of those does not count. Malformed markup is
//! not its concern — `roxmltree` reports that afterwards as it always did.

use crate::ConversionError;

/// Default nesting limit; see the module docs.
pub const DEFAULT_MAX_DEPTH: usize = 512;

/// `DOCLING_RS_MAX_XML_DEPTH`, else [`DEFAULT_MAX_DEPTH`].
pub fn max_depth() -> usize {
    static CAP: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CAP.get_or_init(|| {
        docling_core::env::parse::<usize>("DOCLING_RS_MAX_XML_DEPTH")
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_MAX_DEPTH)
    })
}

/// The deepest element nesting in `xml`, or `None` as soon as it exceeds
/// `limit` (the scan stops there).
pub fn nesting_depth(xml: &str, limit: usize) -> Option<usize> {
    let b = xml.as_bytes();
    let n = b.len();
    let (mut depth, mut max) = (0usize, 0usize);
    let mut i = 0;
    let find = |from: usize, pat: &[u8]| -> usize {
        // Position just past the first `pat` at or after `from`, or `n`.
        let mut j = from;
        while j + pat.len() <= n {
            if &b[j..j + pat.len()] == pat {
                return j + pat.len();
            }
            j += 1;
        }
        n
    };
    while i < n {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"<!--") {
            i = find(i + 4, b"-->");
        } else if b[i..].starts_with(b"<![CDATA[") {
            i = find(i + 9, b"]]>");
        } else if b[i..].starts_with(b"<?") {
            i = find(i + 2, b"?>");
        } else if b[i..].starts_with(b"<!") {
            // DOCTYPE (or any other declaration): to its `>`, skipping an
            // internal subset `[ … ]`, whose entity declarations carry their
            // own `<` and `>`.
            let mut j = i + 2;
            let mut bracket = 0usize;
            while j < n {
                match b[j] {
                    b'[' => bracket += 1,
                    b']' => bracket = bracket.saturating_sub(1),
                    b'>' if bracket == 0 => break,
                    _ => {}
                }
                j += 1;
            }
            i = j + 1;
        } else if b[i..].starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = find(i + 2, b">");
        } else {
            // Start tag (or empty-element tag): walk to its `>`, honouring
            // quoted attribute values.
            let mut j = i + 1;
            let mut quote: Option<u8> = None;
            let mut prev = b'<';
            while j < n {
                let c = b[j];
                match quote {
                    Some(q) if c == q => quote = None,
                    Some(_) => {}
                    None if c == b'"' || c == b'\'' => quote = Some(c),
                    None if c == b'>' => break,
                    None => {}
                }
                prev = c;
                j += 1;
            }
            if prev != b'/' {
                depth += 1;
                if depth > limit {
                    return None;
                }
                max = max.max(depth);
            }
            i = j + 1;
        }
    }
    Some(max)
}

/// Reject `xml` when it nests deeper than [`max_depth`]. `what` names the
/// input in the error (a part path, a format).
pub fn check(xml: &str, what: &str) -> Result<(), ConversionError> {
    let limit = max_depth();
    match nesting_depth(xml, limit) {
        Some(_) => Ok(()),
        None => Err(ConversionError::Parse(format!(
            "{what}: XML nested deeper than {limit} elements (DOCLING_RS_MAX_XML_DEPTH)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_elements_and_skips_non_element_markup() {
        let xml = r#"<?xml version="1.0"?>
<!DOCTYPE a [ <!ENTITY e "<b><c><d>"> ]>
<!-- <x><y><z> -->
<a x="<q><r>" y='<s>'><![CDATA[<t><u><v>]]><b/><c><d>text</d></c></a>"#;
        assert_eq!(nesting_depth(xml, 100), Some(3));
        assert_eq!(nesting_depth("<a><b><c/></b></a>", 100), Some(2));
        assert_eq!(nesting_depth("", 100), Some(0));
    }

    #[test]
    fn stops_at_the_limit() {
        let deep: String = "<t>".repeat(600) + &"</t>".repeat(600);
        assert_eq!(nesting_depth(&deep, 512), None);
        assert_eq!(nesting_depth(&deep, 600), Some(600));
        assert!(check(&deep, "test").is_err());
        assert!(check("<a><b/></a>", "test").is_ok());
    }
}
