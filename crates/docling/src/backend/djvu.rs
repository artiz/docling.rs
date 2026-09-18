//! DjVu input (`.djvu` / `.djv`) — a docling.rs extension (#434).
//!
//! DjVu is a multi-page scanned-document format (JB2 bilevel masks + IW44
//! wavelet colour layers) that almost always carries a hidden per-page OCR
//! **text layer** — that is the whole point of DjVu: small, searchable scans.
//! Upstream docling has no DjVu reader, so this is a docling.rs extension in the
//! spirit of Visio (#214), SVG (#212) and RTF (#209).
//!
//! Decoding is pure Rust through the `djvu-rs` crate (MIT, no GPL
//! dependencies, written from the public DjVu v3 spec): no DjVuLibre binary,
//! no subprocess, no temp files — so it runs identically in every build,
//! including wasm. Mirroring the pdf / pdf-text split, the **text layer is the
//! default**: it is the DjVu-native content, deterministic, and needs no
//! models. When no selected page carries a text layer and the ML pipeline is
//! built, the caller rasterizes ([`rasterize_pages`]) and OCRs instead; without
//! ML a scan-only DjVu degrades to an empty document with a warning.

use djvu_rs::DjVuDocument;
use docling_core::{DoclingDocument, Node};

use crate::error::ConversionError;
use crate::source::SourceDocument;

/// Whether `bytes` is a DjVu document, by its `AT&TFORM` IFF85 wrapper and a
/// DjVu form type — single page (`DJVU`), multi-page bundle (`DJVM`), included
/// component (`DJVI`) or thumbnails (`THUM`). Checked by content, not
/// extension, like the other magic sniffs (serve uses it for uploads whose
/// name and Content-Type say nothing).
pub fn looks_like_djvu(bytes: &[u8]) -> bool {
    bytes.len() >= 16
        && &bytes[0..8] == b"AT&TFORM"
        && matches!(&bytes[12..16], b"DJVM" | b"DJVU" | b"DJVI" | b"THUM")
}

fn parse(bytes: &[u8]) -> Result<DjVuDocument, ConversionError> {
    DjVuDocument::parse(bytes).map_err(|e| ConversionError::Parse(format!("djvu: {e}")))
}

/// Resolve a 1-based inclusive page window against the document, mirroring the
/// PDF `pages` semantics (#80): `None` = all, the end clamps, a start past the
/// end errors.
fn resolve_range(
    range: Option<(usize, usize)>,
    total: usize,
) -> Result<(usize, usize), ConversionError> {
    match range {
        None => Ok((1, total)),
        Some((first, last)) => {
            if first == 0 || last < first {
                return Err(ConversionError::Parse(format!(
                    "djvu: invalid page range {first}-{last} (pages are 1-based, first <= last)"
                )));
            }
            if first > total {
                return Err(ConversionError::Parse(format!(
                    "djvu: page range {first}-{last} is outside the document ({total} page(s))"
                )));
            }
            Ok((first, last.min(total)))
        }
    }
}

/// Split a page's text layer into paragraphs. The layer is a zone tree
/// (page → region → paragraph → line → word); its plain-text rendering
/// separates paragraphs by a blank line and physical lines by `\n`. A
/// paragraph is those lines re-joined with a single space, whitespace
/// collapsed — which reflows the scan's line breaks back into running prose.
fn paragraphs(page: &str) -> Vec<String> {
    page.split("\n\n")
        .filter_map(|block| {
            let joined = block.split_whitespace().collect::<Vec<_>>().join(" ");
            (!joined.is_empty()).then_some(joined)
        })
        .collect()
}

/// Extract the DjVu text layer into a document: each page's paragraphs as
/// `Paragraph` nodes, `PageBreak` between pages. The returned document has no
/// paragraphs when no selected page carries a text layer — the caller decides
/// whether to fall back to rasterize + OCR.
pub fn convert_text_layer(
    source: &SourceDocument,
    range: Option<(usize, usize)>,
) -> Result<DoclingDocument, ConversionError> {
    let djvu = parse(&source.bytes)?;
    let (first, last) = resolve_range(range, djvu.page_count())?;

    let mut doc = DoclingDocument::new(&source.name);
    for page_no in first..=last {
        if page_no > first {
            doc.push(Node::PageBreak);
        }
        let page = djvu
            .page(page_no - 1)
            .map_err(|e| ConversionError::Parse(format!("djvu: page {page_no}: {e}")))?;
        let text = page
            .text()
            .map_err(|e| ConversionError::Parse(format!("djvu: page {page_no} text layer: {e}")))?;
        for para in paragraphs(text.as_deref().unwrap_or("")) {
            doc.push(Node::Paragraph { text: para });
        }
    }
    Ok(doc)
}

/// True when a [`convert_text_layer`] document has no text content — every
/// node is a page break (a scan-only DjVu). Signals the caller to try OCR.
pub fn is_text_layer_empty(doc: &DoclingDocument) -> bool {
    !doc.nodes
        .iter()
        .any(|n| matches!(n, Node::Paragraph { .. }))
}

/// Rasterize the selected pages to PNG for the image/OCR pipeline (the fallback
/// for a scan-only DjVu). Each page renders through `djvu-rs` fitted into a
/// `DOCLING_RS_DJVU_RENDER_PX` box (2500 px, ~OCR's sweet spot) so a 600-dpi
/// scan does not hand OCR a 30-megapixel bitmap. Only built with the ML
/// pipeline (`pdf`), its sole consumer.
#[cfg(feature = "pdf")]
pub fn rasterize_pages(
    bytes: &[u8],
    range: Option<(usize, usize)>,
) -> Result<Vec<Vec<u8>>, ConversionError> {
    use djvu_rs::djvu_render::{render_pixmap, RenderOptions};
    use std::io::Cursor;

    let max_side: u32 = docling_core::env::parse("DOCLING_RS_DJVU_RENDER_PX").unwrap_or(2500);

    let djvu = parse(bytes)?;
    let (first, last) = resolve_range(range, djvu.page_count())?;

    let mut pngs = Vec::with_capacity(last - first + 1);
    for page_no in first..=last {
        let page = djvu
            .page(page_no - 1)
            .map_err(|e| ConversionError::Parse(format!("djvu: page {page_no}: {e}")))?;
        let opts = RenderOptions::fit_to_box(page, max_side, max_side);
        let pm = render_pixmap(page, &opts)
            .map_err(|e| ConversionError::Parse(format!("djvu: rendering page {page_no}: {e}")))?;
        let rgba = image::RgbaImage::from_raw(pm.width, pm.height, pm.data).ok_or_else(|| {
            ConversionError::Parse(format!("djvu: page {page_no}: pixmap size mismatch"))
        })?;
        // OCR/layout are trained on opaque paper: drop the alpha channel.
        let rgb = image::DynamicImage::ImageRgba8(rgba).to_rgb8();
        let mut png = Vec::new();
        rgb.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| ConversionError::Parse(format!("djvu: encoding page {page_no}: {e}")))?;
        pngs.push(png);
    }
    Ok(pngs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::InputFormat;

    #[test]
    fn magic_recognizes_the_djvu_form_types() {
        let mut multi = b"AT&TFORM\x00\x00\x03\x3cDJVM".to_vec();
        multi.extend_from_slice(b"rest");
        assert!(looks_like_djvu(&multi));
        assert!(looks_like_djvu(b"AT&TFORM\x00\x00\x00\x10DJVUxxxx"));
        assert!(looks_like_djvu(b"AT&TFORM\x00\x00\x00\x10DJVIxxxx"));
        // Not DjVu: PDF, a bare IFF, too short.
        assert!(!looks_like_djvu(b"%PDF-1.7"));
        assert!(!looks_like_djvu(b"AT&TFORM\x00\x00\x00\x10PORTxxxx"));
        assert!(!looks_like_djvu(b"AT&TFORM"));
    }

    #[test]
    fn paragraphs_reflow_blank_line_blocks() {
        // Blank-line-separated blocks become paragraphs; physical line breaks
        // inside a block collapse to single spaces; trailing spaces vanish.
        let page = "First line \nsecond line \n\nNext paragraph \n\n\n  \n";
        assert_eq!(
            paragraphs(page),
            vec![
                "First line second line".to_string(),
                "Next paragraph".to_string()
            ]
        );
        assert!(paragraphs("   \n\n \n").is_empty());
    }

    #[test]
    fn range_resolution_matches_pdf_semantics() {
        assert_eq!(resolve_range(None, 5).unwrap(), (1, 5));
        assert_eq!(resolve_range(Some((2, 3)), 5).unwrap(), (2, 3));
        assert_eq!(resolve_range(Some((2, 99)), 5).unwrap(), (2, 5)); // end clamps
        assert!(resolve_range(Some((0, 3)), 5).is_err()); // 0 is not a page
        assert!(resolve_range(Some((3, 2)), 5).is_err()); // reversed
        assert!(resolve_range(Some((9, 9)), 5).is_err()); // start past end
    }

    /// The bundled 3-page fixture: the text layer comes out as the
    /// blank-line-delimited paragraphs of each page, `--pages` narrows it, and
    /// a corrupt file is a clean error, all in-process (no external binary).
    #[test]
    fn text_layer_of_the_fixture_extracts_in_process() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/djvu/sources/example.djvu"
        );
        let bytes = std::fs::read(path).expect("fixture exists");
        let src = SourceDocument::from_bytes("example", InputFormat::Djvu, bytes);

        let doc = convert_text_layer(&src, None).expect("converts");
        assert!(!is_text_layer_empty(&doc));
        let md = doc.export_to_markdown();
        assert!(md.contains("RGB Image CMYK Image CMYK Graphic"), "{md:?}");
        assert!(md.contains("Black RGB Gray RGB Neutral CIELAB"), "{md:?}");
        assert_eq!(
            doc.nodes
                .iter()
                .filter(|n| matches!(n, Node::PageBreak))
                .count(),
            2,
            "3 pages -> 2 page breaks"
        );

        // Page 2 alone: no page break, not page 1's specimen table.
        let p2 = convert_text_layer(&src, Some((2, 2))).expect("converts");
        assert!(!p2.nodes.iter().any(|n| matches!(n, Node::PageBreak)));
        assert!(!p2.export_to_markdown().contains("RGB Image CMYK Image"));

        assert!(convert_text_layer(&src, Some((9, 9))).is_err());
        let junk = SourceDocument::from_bytes("junk", InputFormat::Djvu, b"AT&TFORM junk".to_vec());
        assert!(convert_text_layer(&junk, None).is_err());
    }
}
