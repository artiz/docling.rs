//! Content-based format detection for the retry after a failed conversion
//! (#556).
//!
//! The format comes from the file extension, and a document converts through
//! that format's backend with no look at its content first — the common case
//! pays nothing. Only when that conversion fails does [`detect`] read the
//! bytes, and when they are evidently another format the converter retries
//! once with it. Real-world archives carry plenty of mislabelled files: in one
//! corpus of 1,573 legacy `.doc` files, 185 were something else — RTF, Word
//! templates and OOXML packages saved as `.doc`, HTML exported "as Word" —
//! which used to fail with "not a compound file".
//!
//! The markers are the formats' own signatures, nothing heuristic: the RTF
//! group opener, the PDF header, a ZIP package's part names (OOXML, ODF and
//! EPUB `mimetype`), an OLE compound file's stream names, HTML markup. A
//! file none of them recognise is left with its original error.

use std::io::{Cursor, Read};

use crate::format::InputFormat;

/// The format `bytes` evidently are, if any of the signatures matches.
pub(crate) fn detect(bytes: &[u8]) -> Option<InputFormat> {
    if looks_like_pdf(bytes) {
        return Some(InputFormat::Pdf);
    }
    let text = text_head(bytes);
    if text.starts_with(b"{\\rtf") {
        return Some(InputFormat::Rtf);
    }
    // A flat Word for Windows 1.x / 2.0 file (#566): no container to look
    // into, its FIB is the first bytes.
    if crate::backend::doc::is_word2(bytes) {
        return Some(InputFormat::Doc);
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return zip_kind(bytes);
    }
    if crate::backend::cfb::CompoundFile::detect(bytes) {
        return cfb_kind(bytes);
    }
    if looks_like_html(text) {
        return Some(InputFormat::Html);
    }
    None
}

/// [`detect`] plus the formats a failed conversion never needs re-routing
/// to but a nameless input (stdin, a `data:` URL, an extensionless URL
/// with a generic `Content-Type`) does: the raster image headers and DjVu.
pub(crate) fn detect_any(bytes: &[u8]) -> Option<InputFormat> {
    detect(bytes)
        .or_else(|| looks_like_image(bytes).then_some(InputFormat::Image))
        .or_else(|| crate::backend::looks_like_djvu(bytes).then_some(InputFormat::Djvu))
}

/// Whether `bytes` are text (#671): the fallback route for an unknown or
/// missing extension. Not empty, no NUL byte, and either valid UTF-8 (a
/// BOM allowed) with control characters other than tab, newline, carriage
/// return and form feed under one per cent, or — a single-byte legacy
/// encoding — the same test on the bytes themselves with the high half
/// counted as letters. Random bytes, archives and binaries fail on NULs and
/// control characters; a `.log` in windows-1251 passes.
pub(crate) fn looks_like_text(bytes: &[u8]) -> bool {
    let b = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if b.is_empty() || b.contains(&0) {
        return false;
    }
    let is_control = |c: u32| c < 0x20 && !matches!(c, 0x09 | 0x0A | 0x0D | 0x0C) || c == 0x7F;
    let (total, control) = match std::str::from_utf8(b) {
        Ok(s) => (
            s.chars().count(),
            s.chars().filter(|&c| is_control(c as u32)).count(),
        ),
        Err(_) => (b.len(), b.iter().filter(|&&c| is_control(c as u32)).count()),
    };
    control * 100 < total
}

/// The magic of the raster formats the image pipeline decodes: PNG, JPEG,
/// GIF, BMP, TIFF (both byte orders), WebP.
fn looks_like_image(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"\xFF\xD8\xFF")
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || bytes.starts_with(b"BM")
        || bytes.starts_with(b"II*\0")
        || bytes.starts_with(b"MM\0*")
        || (bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP")
}

/// A PDF header within the first KiB — where readers (pdfium, docling-parse)
/// accept it after leading junk.
pub(crate) fn looks_like_pdf(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(1024)]
        .windows(5)
        .any(|w| w == b"%PDF-")
}

/// The first KiB past a UTF-8 BOM and leading whitespace.
fn text_head(bytes: &[u8]) -> &[u8] {
    let b = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let start = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    &b[start..b.len().min(start + 1024)]
}

/// Markup that opens an HTML document: a doctype, `<html>`, or a fragment
/// whose head names one (Word's "Save as Web Page" starts with comments and
/// conditional markup before `<html>`).
fn looks_like_html(head: &[u8]) -> bool {
    if head.first() != Some(&b'<') {
        return false;
    }
    let lower = head.to_ascii_lowercase();
    let has = |needle: &[u8]| lower.windows(needle.len()).any(|w| w == needle);
    has(b"<!doctype html")
        || has(b"<html")
        || lower.starts_with(b"<head")
        || lower.starts_with(b"<body")
}

/// A ZIP package by its parts: OOXML main parts, the ODF / EPUB `mimetype`.
fn zip_kind(bytes: &[u8]) -> Option<InputFormat> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let has = |zip: &zip::ZipArchive<_>, name: &str| zip.file_names().any(|n| n == name);
    if has(&zip, "word/document.xml") {
        return Some(InputFormat::Docx);
    }
    if has(&zip, "xl/workbook.xml") || has(&zip, "xl/workbook.bin") {
        return Some(InputFormat::Xlsx);
    }
    if has(&zip, "ppt/presentation.xml") {
        return Some(InputFormat::Pptx);
    }
    if has(&zip, "visio/document.xml") {
        return Some(InputFormat::Visio);
    }
    let mut mimetype = String::new();
    zip.by_name("mimetype")
        .ok()?
        .take(256)
        .read_to_string(&mut mimetype)
        .ok()?;
    let mime = mimetype.trim();
    match mime {
        "application/epub+zip" => Some(InputFormat::Epub),
        _ if mime.starts_with("application/vnd.oasis.opendocument.text") => Some(InputFormat::Odt),
        _ if mime.starts_with("application/vnd.oasis.opendocument.spreadsheet") => {
            Some(InputFormat::Ods)
        }
        _ if mime.starts_with("application/vnd.oasis.opendocument.presentation") => {
            Some(InputFormat::Odp)
        }
        _ => None,
    }
}

/// An OLE compound file by its root streams: Word, Excel, PowerPoint,
/// Outlook. Root only — an e-mail carrying a Word attachment holds a
/// `WordDocument` stream too, one storage down.
fn cfb_kind(bytes: &[u8]) -> Option<InputFormat> {
    let cfb = crate::backend::cfb::CompoundFile::open(bytes)?;
    let root: Vec<&str> = cfb
        .children_of(None)
        .into_iter()
        .filter(|&i| !cfb.is_storage(i))
        .map(|i| cfb.entry_name(i))
        .collect();
    let has = |name: &str| root.contains(&name);
    if has("WordDocument") {
        Some(InputFormat::Doc)
    } else if has("Workbook") || has("Book") {
        Some(InputFormat::Xls)
    } else if has("PowerPoint Document") {
        Some(InputFormat::Ppt)
    } else if has("__properties_version1.0") {
        Some(InputFormat::Email)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        let mut w = zip::ZipWriter::new(&mut out);
        for (name, data) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
        out.into_inner()
    }

    #[test]
    fn signatures() {
        assert_eq!(
            detect(b"\xEF\xBB\xBF \r\n{\\rtf1\\ansi x}"),
            Some(InputFormat::Rtf)
        );
        assert_eq!(detect(b"junk%PDF-1.7\n"), Some(InputFormat::Pdf));
        assert_eq!(
            detect(b"<!-- saved --><!DOCTYPE html><html><body>x</body></html>"),
            Some(InputFormat::Html)
        );
        assert_eq!(
            detect(b"<html xmlns:o=\"urn:schemas-microsoft-com:office:office\">"),
            Some(InputFormat::Html)
        );
        assert_eq!(detect(b"plain text"), None);
        assert_eq!(detect(b"<?xml version=\"1.0\"?><article/>"), None);
        assert_eq!(detect(b""), None);
    }

    #[test]
    fn zip_packages_by_their_parts() {
        let docx = zip_with(&[
            ("[Content_Types].xml", b"<Types/>"),
            ("word/document.xml", b"<w/>"),
        ]);
        assert_eq!(detect(&docx), Some(InputFormat::Docx));
        let xlsx = zip_with(&[("xl/workbook.xml", b"<w/>")]);
        assert_eq!(detect(&xlsx), Some(InputFormat::Xlsx));
        let pptx = zip_with(&[("ppt/presentation.xml", b"<p/>")]);
        assert_eq!(detect(&pptx), Some(InputFormat::Pptx));
        let odt = zip_with(&[("mimetype", b"application/vnd.oasis.opendocument.text")]);
        assert_eq!(detect(&odt), Some(InputFormat::Odt));
        let ods = zip_with(&[(
            "mimetype",
            b"application/vnd.oasis.opendocument.spreadsheet",
        )]);
        assert_eq!(detect(&ods), Some(InputFormat::Ods));
        let epub = zip_with(&[("mimetype", b"application/epub+zip")]);
        assert_eq!(detect(&epub), Some(InputFormat::Epub));
        let other = zip_with(&[("readme.txt", b"hi")]);
        assert_eq!(detect(&other), None);
    }

    /// A nameless input (stdin, a `data:` URL, an extensionless URL or
    /// file) is typed by its signature alone — text without markup stays
    /// unknown rather than guessed.
    #[test]
    fn detect_any_reads_signatures() {
        let docx = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/data/docx/sources/lorem_ipsum.docx"),
        )
        .unwrap();
        assert_eq!(detect_any(&docx), Some(InputFormat::Docx));
        assert_eq!(detect_any(b"%PDF-1.7\n%..."), Some(InputFormat::Pdf));
        for image in [
            &b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"[..],
            b"\xFF\xD8\xFF\xE0\0\x10JFIF",
            b"RIFF\0\0\0\0WEBPVP8 ",
        ] {
            assert_eq!(detect_any(image), Some(InputFormat::Image));
        }
        assert_eq!(
            detect_any(b"<!DOCTYPE html><html><body>x</body></html>"),
            Some(InputFormat::Html)
        );
        assert_eq!(detect_any(b"{\\rtf1\\ansi x}"), Some(InputFormat::Rtf));
        assert_eq!(detect_any(b"just some words"), None);
        assert_eq!(detect_any(b""), None);
        // A WAV file is a RIFF container too — not an image.
        assert_eq!(detect_any(b"RIFF\0\0\0\0WAVEfmt "), None);
    }
}
