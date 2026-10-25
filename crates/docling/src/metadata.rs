//! The file's own metadata (#679), read into
//! [`DocumentMetadata`](docling_core::DocumentMetadata) when the converter's
//! output mode will show it (`DocumentConverter::metadata_output`): an
//! Office package's `docProps/core.xml` and `app.xml`, an ODF `meta.xml`
//! (or a flat document's `office:meta`), an EPUB's OPF Dublin Core, a
//! photo's EXIF (JPEG, PNG, TIFF, WebP, HEIF), an audio or video file's
//! tags, a PDF's Info dictionary. Everything is pure Rust and reads
//! headers only; a file that carries nothing leaves the metadata empty, a
//! file that cannot be read for it is not an error.
//!
//! What MarkItDown shows for the same inputs: its EPUB converter prints
//! `dc:title/creator/language/publisher/date/description/identifier`, its
//! image and audio converters exiftool's `Title`, `Caption`, `Description`,
//! `Keywords`, `Artist`, `Author`, `DateTimeOriginal`, `CreateDate`,
//! `GPSPosition`, `Album`, `Genre`, `Track`, … — every one of those has its
//! place here (the common fields, `exif`, `audio`, `gps`), so the
//! MarkItDown facade can print MarkItDown's block from this.

use std::collections::BTreeMap;
use std::io::Cursor;

use docling_core::{DocumentMetadata, GpsPosition};
use roxmltree::{Document, Node};

use crate::backend::ooxml::Package;
use crate::{InputFormat, SourceDocument};

/// The metadata of `source`, by its format; empty for formats that carry
/// none (text, HTML, CSV, …) and for files that cannot be read.
pub(crate) fn read(source: &SourceDocument, password: Option<&str>) -> DocumentMetadata {
    let bytes = &source.bytes;
    match source.format {
        InputFormat::Docx | InputFormat::Pptx | InputFormat::Xlsx | InputFormat::Visio => {
            Package::open_slice(bytes)
                .map(|mut p| from_ooxml(&mut p))
                .unwrap_or_default()
        }
        InputFormat::Odt | InputFormat::Ods | InputFormat::Odp => {
            match Package::open_slice(bytes) {
                Some(mut pkg) => pkg
                    .read("meta.xml")
                    .map(|xml| from_odf_xml(&xml))
                    .unwrap_or_default(),
                // Flat ODF: the metadata is in the document's own DOM.
                None => source
                    .text()
                    .ok()
                    .map(|xml| from_odf_xml(&xml))
                    .unwrap_or_default(),
            }
        }
        InputFormat::Epub => Package::open_slice(bytes)
            .and_then(|mut pkg| {
                let container = pkg.read("META-INF/container.xml")?;
                let opf = pkg.read(&crate::backend::epub::rootfile_path(&container)?)?;
                Some(from_opf(&opf))
            })
            .unwrap_or_default(),
        InputFormat::Image => from_exif(bytes),
        #[cfg(any(feature = "pdf", feature = "pdf-text"))]
        InputFormat::Pdf => from_pdf_info(docling_pdf::info::document_info(bytes, password)),
        #[cfg(feature = "asr")]
        InputFormat::Audio | InputFormat::Video => {
            from_audio_tags(docling_asr::audio::probe_metadata(bytes, &source.name))
        }
        _ => {
            let _ = password;
            DocumentMetadata::default()
        }
    }
}

fn clean(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// `a, b; c` → `[a, b, c]`.
fn split_list(s: &str) -> Vec<String> {
    s.split([',', ';']).filter_map(clean).collect()
}

fn text_of(node: Node) -> Option<String> {
    clean(node.text().unwrap_or_default())
}

// ---------------------------------------------------------------------------
// OOXML: docProps/core.xml (Dublin Core + cp:*) and docProps/app.xml
// ---------------------------------------------------------------------------

/// The package's core and application properties.
pub(crate) fn from_ooxml<B: AsRef<[u8]>>(pkg: &mut Package<B>) -> DocumentMetadata {
    let mut m = DocumentMetadata::default();
    if let Some(core) = pkg.read("docProps/core.xml") {
        if let Ok(dom) = Document::parse(&core) {
            for n in dom.root_element().children().filter(|n| n.is_element()) {
                let Some(v) = text_of(n) else { continue };
                match n.tag_name().name() {
                    "title" => m.title = Some(v),
                    "subject" => m.subject = Some(v),
                    "creator" => m.authors = split_list(&v),
                    "keywords" => m.keywords = split_list(&v),
                    "description" => m.description = Some(v),
                    "lastModifiedBy" => m.last_modified_by = Some(v),
                    "created" => m.created = Some(v),
                    "modified" => m.modified = Some(v),
                    "language" => m.language = Some(v),
                    "identifier" => m.identifier = Some(v),
                    "revision" => {
                        m.properties.insert("revision".into(), v);
                    }
                    "category" => {
                        m.properties.insert("category".into(), v);
                    }
                    "contentStatus" => {
                        m.properties.insert("content_status".into(), v);
                    }
                    "version" => {
                        m.properties.insert("version".into(), v);
                    }
                    "lastPrinted" => {
                        m.properties.insert("last_printed".into(), v);
                    }
                    _ => {}
                }
            }
        }
    }
    if let Some(app) = pkg.read("docProps/app.xml") {
        if let Ok(dom) = Document::parse(&app) {
            for n in dom.root_element().children().filter(|n| n.is_element()) {
                let Some(v) = text_of(n) else { continue };
                let key = match n.tag_name().name() {
                    "Application" => {
                        m.creator_tool = Some(v);
                        continue;
                    }
                    "AppVersion" => "application_version",
                    "Company" => "company",
                    "Manager" => "manager",
                    "Template" => "template",
                    "Pages" => "pages",
                    "Words" => "words",
                    "Characters" => "characters",
                    "Lines" => "lines",
                    "Paragraphs" => "paragraphs",
                    "Slides" => "slides",
                    "Notes" => "notes",
                    "TotalTime" => "total_time",
                    "PresentationFormat" => "presentation_format",
                    _ => continue,
                };
                m.properties.insert(key.into(), v);
            }
        }
    }
    m
}

// ---------------------------------------------------------------------------
// ODF: meta.xml / a flat document's office:meta
// ---------------------------------------------------------------------------

/// `office:meta` of `xml` (a `meta.xml` or a whole flat document).
pub(crate) fn from_odf_xml(xml: &str) -> DocumentMetadata {
    let mut m = DocumentMetadata::default();
    let Ok(dom) = Document::parse(xml) else {
        return m;
    };
    let Some(meta) = dom.descendants().find(|n| {
        n.is_element()
            && n.tag_name().name() == "meta"
            && n.parent()
                .is_some_and(|p| p.is_root() || p.parent().is_some_and(|g| g.is_root()))
    }) else {
        return m;
    };
    for n in meta.children().filter(|n| n.is_element()) {
        let name = n.tag_name().name();
        if name == "document-statistic" {
            for a in n.attributes() {
                let key = a.name().replace('-', "_");
                let key = key.strip_suffix("_count").unwrap_or(&key);
                let key = match key {
                    "page" => "pages".to_string(),
                    "word" => "words".to_string(),
                    "character" => "characters".to_string(),
                    "paragraph" => "paragraphs".to_string(),
                    "table" => "tables".to_string(),
                    "image" => "images".to_string(),
                    "object" => "objects".to_string(),
                    "cell" => "cells".to_string(),
                    "sheet" => "sheets".to_string(),
                    other => other.to_string(),
                };
                if let Some(v) = clean(a.value()) {
                    m.properties.insert(key, v);
                }
            }
            continue;
        }
        if name == "user-defined" {
            if let (Some(k), Some(v)) = (
                n.attribute(("urn:oasis:names:tc:opendocument:xmlns:meta:1.0", "name"))
                    .or_else(|| {
                        n.attributes()
                            .find(|a| a.name() == "name")
                            .map(|a| a.value())
                    })
                    .and_then(clean),
                text_of(n),
            ) {
                m.properties.insert(k.to_lowercase().replace(' ', "_"), v);
            }
            continue;
        }
        let Some(v) = text_of(n) else { continue };
        match name {
            "generator" => m.creator_tool = Some(v),
            "title" => m.title = Some(v),
            "subject" => m.subject = Some(v),
            "description" => m.description = Some(v),
            "keyword" => m.keywords.push(v),
            "initial-creator" => m.authors.push(v),
            // ODF's dc:creator is the last editor; dc:date the last change.
            "creator" => m.last_modified_by = Some(v),
            "creation-date" => m.created = Some(v),
            "date" => m.modified = Some(v),
            "language" => m.language = Some(v),
            "editing-cycles" => {
                m.properties.insert("editing_cycles".into(), v);
            }
            "editing-duration" => {
                m.properties.insert("editing_duration".into(), v);
            }
            _ => {}
        }
    }
    m
}

// ---------------------------------------------------------------------------
// EPUB: the OPF package's <metadata>
// ---------------------------------------------------------------------------

/// The Dublin Core of an OPF package document.
pub(crate) fn from_opf(opf: &str) -> DocumentMetadata {
    let mut m = DocumentMetadata::default();
    let Ok(dom) = Document::parse(opf) else {
        return m;
    };
    let Some(meta) = dom
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "metadata")
    else {
        return m;
    };
    for n in meta.children().filter(|n| n.is_element()) {
        let name = n.tag_name().name();
        if name == "meta" {
            // EPUB 3 `<meta property="dcterms:modified">`, EPUB 2
            // `<meta name="generator" content="…">`.
            match (
                n.attribute("property"),
                n.attribute("name"),
                n.attribute("content"),
            ) {
                (Some("dcterms:modified"), _, _) => m.modified = text_of(n),
                (Some("dcterms:created"), _, _) => m.created = text_of(n),
                (_, Some("generator"), Some(c)) => m.creator_tool = clean(c),
                _ => {}
            }
            continue;
        }
        let Some(v) = text_of(n) else { continue };
        match name {
            "title" => {
                if m.title.is_none() {
                    m.title = Some(v);
                }
            }
            "creator" => m.authors.push(v),
            "contributor" => {
                m.properties
                    .entry("contributors".into())
                    .and_modify(|c| {
                        c.push_str(", ");
                        c.push_str(&v);
                    })
                    .or_insert(v);
            }
            "language" => {
                if m.language.is_none() {
                    m.language = Some(v);
                }
            }
            "publisher" => m.publisher = Some(v),
            "date" => {
                if m.date.is_none() {
                    m.date = Some(v);
                }
            }
            "description" => m.description = Some(v),
            "identifier" => {
                if m.identifier.is_none() {
                    m.identifier = Some(v);
                }
            }
            "subject" => m.keywords.push(v),
            "rights" => m.rights = Some(v),
            _ => {}
        }
    }
    m
}

// ---------------------------------------------------------------------------
// Images: EXIF
// ---------------------------------------------------------------------------

/// The pixel size and the EXIF of an image file: the primary image's tags
/// (IFD0, the Exif IFD, the GPS IFD — not the thumbnail's), as exiftool
/// names and prints them, the Windows `XP*` tags decoded from UTF-16, the
/// GPS position in decimal degrees, and the common fields derived from
/// them (`Artist` → authors, `DateTimeOriginal` → date, `Software` →
/// creator tool, …). Binary tags and pointers are left out.
pub(crate) fn from_exif(bytes: &[u8]) -> DocumentMetadata {
    use exif::{In, Tag, Value};

    let mut m = DocumentMetadata::default();
    if let Ok(reader) = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format() {
        if let Ok((w, h)) = reader.into_dimensions() {
            m.properties.insert("image_size".into(), format!("{w}x{h}"));
        }
    }
    let mut reader = exif::Reader::new();
    reader.continue_on_error(true);
    let exif = match reader.read_from_container(&mut Cursor::new(bytes)) {
        Ok(e) => e,
        Err(exif::Error::PartialResult(p)) => p.into_inner().0,
        Err(_) => return m,
    };
    let mut xp: BTreeMap<&'static str, String> = BTreeMap::new();
    for f in exif.fields() {
        if f.ifd_num != In::PRIMARY {
            continue;
        }
        // Windows Explorer's tags: UTF-16LE in a BYTE array.
        let xp_name = match f.tag.number() {
            0x9c9b => Some("XPTitle"),
            0x9c9c => Some("XPComment"),
            0x9c9d => Some("XPAuthor"),
            0x9c9e => Some("XPKeywords"),
            0x9c9f => Some("XPSubject"),
            _ => None,
        };
        if let (Some(name), Value::Byte(b) | Value::Undefined(b, _)) = (xp_name, &f.value) {
            let units: Vec<u16> = b
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|u| *u != 0)
                .collect();
            if let Some(v) = clean(&String::from_utf16_lossy(&units)) {
                m.exif.insert(name.into(), v.clone());
                xp.insert(name, v);
            }
            continue;
        }
        if f.tag.description().is_none() {
            continue;
        }
        let name = f.tag.to_string();
        if name.ends_with("IFDPointer")
            || name.ends_with("Offset")
            || name.ends_with("ByteCount")
            || matches!(
                name.as_str(),
                "MakerNote" | "ImageUniqueID" | "ComponentsConfiguration"
            )
        {
            continue;
        }
        let text = match &f.value {
            Value::Unknown(..) => continue,
            Value::Undefined(b, _) | Value::Byte(b) if b.len() > 64 => continue,
            Value::Undefined(..)
                if f.tag != Tag::UserComment
                    && f.tag != Tag::ExifVersion
                    && f.tag != Tag::FlashpixVersion =>
            {
                continue
            }
            _ => f.display_value().with_unit(&exif).to_string(),
        };
        let text = text.trim().trim_matches('"').trim().to_string();
        if text.is_empty() || text.len() > 500 || text.contains('\u{0}') {
            continue;
        }
        m.exif.insert(name, text);
    }
    // The position: degrees, minutes, seconds with their hemispheres.
    let dms = |tag: Tag, reference: Tag| -> Option<f64> {
        let v = match &exif.get_field(tag, In::PRIMARY)?.value {
            Value::Rational(r) if r.len() >= 3 => {
                r[0].to_f64() + r[1].to_f64() / 60.0 + r[2].to_f64() / 3600.0
            }
            Value::Rational(r) if !r.is_empty() => r[0].to_f64(),
            _ => return None,
        };
        let south_or_west = exif
            .get_field(reference, In::PRIMARY)
            .map(|f| f.display_value().to_string())
            .is_some_and(|s| s.contains('S') || s.contains('W'));
        Some(if south_or_west { -v } else { v }).filter(|v| v.is_finite())
    };
    if let (Some(lat), Some(lon)) = (
        dms(Tag::GPSLatitude, Tag::GPSLatitudeRef),
        dms(Tag::GPSLongitude, Tag::GPSLongitudeRef),
    ) {
        let altitude = exif
            .get_field(Tag::GPSAltitude, In::PRIMARY)
            .and_then(|f| match &f.value {
                Value::Rational(r) if !r.is_empty() => Some(r[0].to_f64()),
                _ => None,
            })
            .map(|a| {
                let below = exif
                    .get_field(Tag::GPSAltitudeRef, In::PRIMARY)
                    .and_then(|f| f.value.get_uint(0))
                    == Some(1);
                if below {
                    -a
                } else {
                    a
                }
            })
            .map(|a| (a * 100.0).round() / 100.0);
        m.gps = Some(GpsPosition {
            latitude: (lat * 1e6).round() / 1e6,
            longitude: (lon * 1e6).round() / 1e6,
            altitude,
        });
    }
    // The common fields.
    let get = |k: &str| m.exif.get(k).cloned();
    m.title = xp.get("XPTitle").cloned();
    m.subject = xp.get("XPSubject").cloned();
    m.description = get("ImageDescription");
    if let Some(c) = xp.get("XPComment") {
        m.properties.insert("comment".into(), c.clone());
    }
    let mut authors: Vec<String> = get("Artist").map(|a| split_list(&a)).unwrap_or_default();
    if let Some(a) = xp.get("XPAuthor") {
        for a in split_list(a) {
            if !authors.contains(&a) {
                authors.push(a);
            }
        }
    }
    m.authors = authors;
    m.keywords = xp
        .get("XPKeywords")
        .map(|k| split_list(k))
        .unwrap_or_default();
    m.date = get("DateTimeOriginal").map(|d| exif_date_to_iso(&d));
    m.created = get("DateTimeDigitized").map(|d| exif_date_to_iso(&d));
    m.modified = get("DateTime").map(|d| exif_date_to_iso(&d));
    m.creator_tool = get("Software");
    m.rights = get("Copyright");
    m
}

/// kamadak-exif prints a DateTime as `YYYY-MM-DD HH:MM:SS`; the file holds
/// `YYYY:MM:DD HH:MM:SS`. Either becomes `YYYY-MM-DDTHH:MM:SS`.
fn exif_date_to_iso(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 19 && s.as_bytes()[10] == b' ' {
        let date = s[..10].replace(':', "-");
        format!("{date}T{}", &s[11..])
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// PDF: the Info dictionary
// ---------------------------------------------------------------------------

/// `Title`, `Author`, `Subject`, `Keywords`, `Creator`, `Producer`,
/// `CreationDate`, `ModDate` into the common fields; the rest into
/// `properties` by a lower-case name.
#[cfg_attr(not(any(feature = "pdf", feature = "pdf-text")), allow(dead_code))]
pub(crate) fn from_pdf_info(info: Vec<(String, String)>) -> DocumentMetadata {
    let mut m = DocumentMetadata::default();
    for (key, value) in info {
        match key.as_str() {
            "Title" => m.title = Some(value),
            "Author" => m.authors = split_list(&value),
            "Subject" => m.subject = Some(value),
            "Keywords" => m.keywords = split_list(&value),
            "Creator" => {
                m.properties.insert("creator".into(), value);
            }
            "Producer" => m.creator_tool = Some(value),
            "CreationDate" => m.created = Some(value),
            "ModDate" => m.modified = Some(value),
            other => {
                let k = other.trim_start_matches('/').to_lowercase();
                if k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !k.is_empty() {
                    m.properties.insert(k, value);
                }
            }
        }
    }
    m
}

// ---------------------------------------------------------------------------
// Audio / video: the container's tags
// ---------------------------------------------------------------------------

/// The tags under `audio` (as the probe names them) and the common fields
/// they imply: `title`, `artist` → authors, `date`, `comment` →
/// description, `copyright` → rights, `language`, `publisher`.
#[cfg_attr(not(feature = "asr"), allow(dead_code))]
pub(crate) fn from_audio_tags(tags: Vec<(String, String)>) -> DocumentMetadata {
    let mut m = DocumentMetadata::default();
    for (key, value) in tags {
        match key.as_str() {
            "title" => m.title = Some(value.clone()),
            "artist" => m.authors = split_list(&value),
            "date" => m.date = Some(value.clone()),
            "comment" | "description" => {
                if m.description.is_none() {
                    m.description = Some(value.clone());
                }
            }
            "copyright" => m.rights = Some(value.clone()),
            "language" => m.language = Some(value.clone()),
            "publisher" => m.publisher = Some(value.clone()),
            "encoder" => m.creator_tool = Some(value.clone()),
            _ => {}
        }
        m.audio.insert(key, value);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opf_dublin_core_reads_into_the_common_fields() {
        let opf = r#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
          <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
            <meta property="dcterms:modified">2024-12-18T10:53:14Z</meta>
            <meta name="generator" content="Ebook-lib 0.18.1"/>
            <dc:identifier id="id">test-book-id</dc:identifier>
            <dc:title>Test EPUB Document</dc:title>
            <dc:language>en</dc:language>
            <dc:creator id="creator">Test Author</dc:creator>
            <dc:creator>Second Author</dc:creator>
            <dc:subject>Testing</dc:subject>
            <dc:description>A test EPUB document for MarkItDown testing</dc:description>
          </metadata></package>"#;
        let m = from_opf(opf);
        assert_eq!(m.title.as_deref(), Some("Test EPUB Document"));
        assert_eq!(m.authors, ["Test Author", "Second Author"]);
        assert_eq!(m.language.as_deref(), Some("en"));
        assert_eq!(m.identifier.as_deref(), Some("test-book-id"));
        assert_eq!(m.modified.as_deref(), Some("2024-12-18T10:53:14Z"));
        assert_eq!(m.creator_tool.as_deref(), Some("Ebook-lib 0.18.1"));
        assert_eq!(m.keywords, ["Testing"]);
        assert!(m
            .to_front_matter()
            .contains("authors: [Test Author, Second Author]\n"));
    }

    #[test]
    fn odf_meta_reads_creators_dates_and_statistics() {
        let xml = r#"<?xml version="1.0"?>
          <office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
            xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0"
            xmlns:dc="http://purl.org/dc/elements/1.1/">
           <office:meta>
            <meta:generator>LibreOffice/25.8</meta:generator>
            <dc:title>Quarterly</dc:title>
            <meta:initial-creator>Ada</meta:initial-creator>
            <dc:creator>Bob</dc:creator>
            <meta:creation-date>2024-01-02T03:04:05</meta:creation-date>
            <dc:date>2024-02-03T04:05:06</dc:date>
            <meta:keyword>one</meta:keyword><meta:keyword>two</meta:keyword>
            <meta:editing-cycles>7</meta:editing-cycles>
            <meta:document-statistic meta:page-count="3" meta:word-count="120"/>
            <meta:user-defined meta:name="Project">Apollo</meta:user-defined>
           </office:meta></office:document-meta>"#;
        let m = from_odf_xml(xml);
        assert_eq!(m.title.as_deref(), Some("Quarterly"));
        assert_eq!(m.authors, ["Ada"]);
        assert_eq!(m.last_modified_by.as_deref(), Some("Bob"));
        assert_eq!(m.created.as_deref(), Some("2024-01-02T03:04:05"));
        assert_eq!(m.modified.as_deref(), Some("2024-02-03T04:05:06"));
        assert_eq!(m.keywords, ["one", "two"]);
        assert_eq!(m.creator_tool.as_deref(), Some("LibreOffice/25.8"));
        assert_eq!(m.properties["pages"], "3");
        assert_eq!(m.properties["words"], "120");
        assert_eq!(m.properties["editing_cycles"], "7");
        assert_eq!(m.properties["project"], "Apollo");
    }

    #[test]
    fn pdf_info_and_audio_tags_map_to_the_common_fields() {
        let m = from_pdf_info(vec![
            ("Title".into(), "T".into()),
            ("Author".into(), "Ada; Bob".into()),
            ("Producer".into(), "pdfTeX".into()),
            ("CreationDate".into(), "2022-03-14T01:27:27Z".into()),
            ("Custom".into(), "x".into()),
        ]);
        assert_eq!(m.title.as_deref(), Some("T"));
        assert_eq!(m.authors, ["Ada", "Bob"]);
        assert_eq!(m.creator_tool.as_deref(), Some("pdfTeX"));
        assert_eq!(m.created.as_deref(), Some("2022-03-14T01:27:27Z"));
        assert_eq!(m.properties["custom"], "x");

        let m = from_audio_tags(vec![
            ("title".into(), "Shakespeare on Scenery".into()),
            ("artist".into(), "Oscar Wilde".into()),
            ("album".into(), "LibriVox".into()),
            ("duration".into(), "10.03".into()),
        ]);
        assert_eq!(m.title.as_deref(), Some("Shakespeare on Scenery"));
        assert_eq!(m.authors, ["Oscar Wilde"]);
        assert_eq!(m.audio["album"], "LibriVox");
        assert_eq!(m.audio["duration"], "10.03");
    }

    #[test]
    fn exif_dates_become_iso_8601() {
        assert_eq!(
            exif_date_to_iso("2024:01:02 03:04:05"),
            "2024-01-02T03:04:05"
        );
        assert_eq!(
            exif_date_to_iso("2024-01-02 03:04:05"),
            "2024-01-02T03:04:05"
        );
        assert_eq!(exif_date_to_iso("2024"), "2024");
    }
}
