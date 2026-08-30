//! Extract a PDF's outline (bookmarks / table of contents) — the most
//! authoritative heading-hierarchy signal a PDF carries (#302, docling's
//! `docling/utils/pdf_outline.py`).
//!
//! Pure lopdf — no pdfium — so it works wherever the crate compiles and adds
//! nothing to the pipeline unless the heading-hierarchy stage asks for it.
//! Returns a flat, document-ordered list; each entry carries its own 0-based
//! depth so no tree structure is needed for matching. Everything is
//! best-effort: a malformed or encrypted outline yields an empty list, never
//! an error.

use std::collections::{HashMap, HashSet};

use lopdf::{Dictionary, Document, Object, ObjectId};

/// A single PDF bookmark / table-of-contents entry.
#[derive(Clone, Debug)]
pub struct OutlineItem {
    pub title: String,
    /// 0-based depth as reported by the PDF outline; compressed to contiguous
    /// levels by the heading-hierarchy stage.
    pub level: usize,
    /// 1-based target page; `None` when the entry has no resolvable page.
    pub page_no: Option<usize>,
    /// Top-left-origin vertical position of the target on its page, when the
    /// destination view encodes one (XYZ / FitH / FitBH / FitR).
    pub y_top: Option<f32>,
}

/// Parse the outline out of raw PDF bytes. Empty when the document has no
/// outline or it cannot be read.
pub fn extract_outline(bytes: &[u8]) -> Vec<OutlineItem> {
    let Ok(doc) = Document::load_mem(bytes) else {
        return Vec::new();
    };
    let Ok(catalog) = doc.catalog() else {
        return Vec::new();
    };
    let Some(outlines) = catalog.get(b"Outlines").ok().and_then(|o| as_dict(&doc, o)) else {
        return Vec::new();
    };
    let Some(first) = outlines
        .get(b"First")
        .ok()
        .and_then(|o| o.as_reference().ok())
    else {
        return Vec::new();
    };

    // Page object id → 1-based index, for resolving destination pages.
    let page_index: HashMap<ObjectId, usize> = doc
        .get_pages()
        .into_iter()
        .map(|(no, id)| (id, no as usize))
        .collect();

    let mut items = Vec::new();
    // Iterative pre-order walk with a visited guard: real documents nest
    // hundreds of levels deep and malformed ones can cycle through /Next.
    let mut visited: HashSet<ObjectId> = HashSet::new();
    let mut stack: Vec<(ObjectId, usize)> = vec![(first, 0)];
    while let Some((id, level)) = stack.pop() {
        if !visited.insert(id) || items.len() >= 10_000 {
            continue;
        }
        let Some(node) = doc.get_object(id).ok().and_then(|o| o.as_dict().ok()) else {
            continue;
        };
        // Siblings after children on the stack ⇒ push /Next first, /First last.
        if let Some(next) = node.get(b"Next").ok().and_then(|o| o.as_reference().ok()) {
            stack.push((next, level));
        }
        if let Some(child) = node.get(b"First").ok().and_then(|o| o.as_reference().ok()) {
            stack.push((child, level + 1));
        }
        let title = node
            .get(b"Title")
            .ok()
            .and_then(|o| deref(&doc, o))
            .and_then(text_string)
            .unwrap_or_default();
        let title = title.trim();
        if title.is_empty() {
            continue;
        }
        let (page_no, y_top) = destination(&doc, catalog, node, &page_index);
        items.push(OutlineItem {
            title: title.to_string(),
            level,
            page_no,
            y_top,
        });
    }
    items
}

/// Resolve an outline item's target: `/Dest` directly, or the `/A` action's
/// `/D` when the action is a GoTo. Returns `(1-based page, top-left y)`.
fn destination(
    doc: &Document,
    catalog: &Dictionary,
    node: &Dictionary,
    page_index: &HashMap<ObjectId, usize>,
) -> (Option<usize>, Option<f32>) {
    let dest = node
        .get(b"Dest")
        .ok()
        .and_then(|o| deref(doc, o))
        .or_else(|| {
            let action = node.get(b"A").ok().and_then(|o| as_dict_obj(doc, o))?;
            let goto = action
                .get(b"S")
                .ok()
                .and_then(|o| o.as_name().ok())
                .is_none_or(|s| s == b"GoTo");
            if !goto {
                return None;
            }
            action.get(b"D").ok().and_then(|o| deref(doc, o))
        });
    let Some(dest) = dest else {
        return (None, None);
    };
    // A named destination (name or byte string) resolves through the catalog.
    let array = match dest {
        Object::Array(a) => Some(a.clone()),
        Object::Name(n) => named_destination(doc, catalog, n),
        Object::String(s, _) => named_destination(doc, catalog, s),
        _ => None,
    };
    let Some(array) = array else {
        return (None, None);
    };
    dest_array(doc, &array, page_index)
}

/// Decode an explicit destination array: `[page /XYZ left top zoom]`,
/// `[page /FitH top]`, `[page /FitBH top]`, `[page /FitR l b r t]`. Views
/// without a usable vertical (Fit, FitV, FitB, FitBV) yield a page only —
/// exactly docling's `_view_top_index`.
fn dest_array(
    doc: &Document,
    array: &[Object],
    page_index: &HashMap<ObjectId, usize>,
) -> (Option<usize>, Option<f32>) {
    let Some(page_obj) = array.first() else {
        return (None, None);
    };
    let (page_no, page_id) = match page_obj {
        Object::Reference(id) => (page_index.get(id).copied(), Some(*id)),
        // A bare integer is a 0-based page index (seen in the wild).
        Object::Integer(i) if *i >= 0 => (Some(*i as usize + 1), None),
        _ => (None, None),
    };
    let view = array.get(1).and_then(|o| o.as_name().ok());
    let y_index = match view {
        Some(b"XYZ") => Some(3),                   // [page /XYZ left top zoom]
        Some(b"FitH") | Some(b"FitBH") => Some(2), // [page /FitH top]
        Some(b"FitR") => Some(5),                  // [page /FitR left bottom right top]
        _ => None,
    };
    let y_pdf = y_index.and_then(|i| array.get(i)).and_then(as_number);
    let y_top = match (y_pdf, page_id) {
        // PDF y-up → top-left origin needs the page height.
        (Some(y), Some(id)) => page_height(doc, id).map(|h| h - y),
        _ => None,
    };
    (page_no, y_top)
}

/// A page's MediaBox height, honoring inheritance from the page tree.
fn page_height(doc: &Document, page: ObjectId) -> Option<f32> {
    let mut id = page;
    for _ in 0..32 {
        let dict = doc.get_object(id).ok()?.as_dict().ok()?;
        if let Some(Object::Array(mb)) = dict.get(b"MediaBox").ok().and_then(|o| deref(doc, o)) {
            let y0 = mb.get(1).and_then(as_number)?;
            let y1 = mb.get(3).and_then(as_number)?;
            return Some((y1 - y0).abs());
        }
        id = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

/// Resolve a named destination: the PDF 1.1 catalog `/Dests` dictionary, or
/// the `/Names` → `/Dests` name tree. The resolved value may itself be a
/// dictionary wrapping the array under `/D`.
fn named_destination(doc: &Document, catalog: &Dictionary, name: &[u8]) -> Option<Vec<Object>> {
    let value = catalog
        .get(b"Dests")
        .ok()
        .and_then(|o| as_dict(doc, o))
        .and_then(|dests| dests.get(name).ok())
        .and_then(|o| deref(doc, o))
        .cloned()
        .or_else(|| {
            let names = catalog.get(b"Names").ok().and_then(|o| as_dict(doc, o))?;
            let tree = names.get(b"Dests").ok().and_then(|o| deref(doc, o))?;
            name_tree_lookup(doc, tree, name, 0)
        })?;
    match value {
        Object::Array(a) => Some(a),
        Object::Dictionary(d) => match d.get(b"D").ok().and_then(|o| deref(doc, o)) {
            Some(Object::Array(a)) => Some(a.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Look a key up in a name tree (`/Names` leaf arrays, `/Kids` interior
/// nodes). Depth-bounded; ignores the `/Limits` optimization and just walks —
/// outlines are read once per conversion.
fn name_tree_lookup(doc: &Document, node: &Object, key: &[u8], depth: usize) -> Option<Object> {
    if depth > 16 {
        return None;
    }
    let dict = as_dict_obj(doc, node)?;
    if let Some(Object::Array(pairs)) = dict.get(b"Names").ok().and_then(|o| deref(doc, o)) {
        for pair in pairs.chunks(2) {
            if let [Object::String(k, _), v] = pair {
                if k == key {
                    return deref(doc, v).cloned();
                }
            }
        }
    }
    if let Some(Object::Array(kids)) = dict.get(b"Kids").ok().and_then(|o| deref(doc, o)) {
        for kid in kids {
            if let Some(found) = name_tree_lookup(doc, kid, key, depth + 1) {
                return Some(found);
            }
        }
    }
    None
}

/// Decode a PDF text string: UTF-16BE with a `FE FF` BOM, else
/// PDFDocEncoding (treated as Latin-1 — identical for the printable range).
fn text_string(obj: &Object) -> Option<String> {
    let Object::String(bytes, _) = obj else {
        return None;
    };
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return Some(String::from_utf16_lossy(&units));
    }
    Some(bytes.iter().map(|&b| b as char).collect())
}

fn as_number(obj: &Object) -> Option<f32> {
    match obj {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

fn deref<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Object> {
    match obj {
        Object::Reference(id) => doc.get_object(*id).ok(),
        other => Some(other),
    }
}

fn as_dict<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Dictionary> {
    deref(doc, obj)?.as_dict().ok()
}

fn as_dict_obj<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Dictionary> {
    as_dict(doc, obj)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shared corpus fixtures live at the repo root (CLAUDE.md); tests run
    /// with CWD = the crate dir.
    fn fixture(name: &str) -> Option<Vec<u8>> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/data/pdf/sources")
            .join(name);
        std::fs::read(p).ok()
    }

    #[test]
    fn reads_a_real_arxiv_outline() {
        // DocLayNet paper: a real multi-level outline with XYZ destinations.
        let Some(bytes) = fixture("2206.01062.pdf") else {
            eprintln!("skipping: corpus fixture not present");
            return;
        };
        let items = extract_outline(&bytes);
        // The DocLayNet paper's outline is flat (8 top-level sections), in
        // document order, with XYZ destinations resolving page and position.
        assert!(items.len() >= 8, "expected the paper's sections");
        assert_eq!(items[0].title, "Abstract");
        assert_eq!(items[0].page_no, Some(1));
        assert!(items[0].y_top.is_some(), "XYZ top resolves");
        assert!(items.iter().all(|i| i.level == 0));
        assert!(items.iter().any(|i| i.title == "6 Conclusion"));
        assert!(
            items.iter().all(|i| i.page_no.is_some()),
            "every entry's target page resolves"
        );
    }

    #[test]
    fn no_outline_is_an_empty_list() {
        let Some(bytes) = fixture("multi_page.pdf") else {
            eprintln!("skipping: corpus fixture not present");
            return;
        };
        // Whether or not this fixture carries an outline, the call must not
        // fail; garbage input must also yield an empty list, never a panic.
        let _ = extract_outline(&bytes);
        assert!(extract_outline(b"%PDF-1.4 not really a pdf").is_empty());
        assert!(extract_outline(&[]).is_empty());
    }
}
