//! MHTML (`.mhtml`/`.mht`) backend — docling's `InputFormat.MHTML`, which
//! docling#4184 added inside its HTML backend (the archive is unwrapped, the
//! root part is converted as HTML). This module is the unwrapping half; the
//! page itself goes through the HTML backend like docling's does.
//!
//! An MHTML archive is a MIME message ([RFC 2557], which `mail-parser`
//! conforms to): a `multipart/related` structure whose root part is the saved
//! page's `text/html`, with its resources (images, CSS, fonts) as sibling
//! parts addressed by `Content-Location` (the resource's original URL) or
//! `Content-ID` (referenced from the HTML as `cid:...`). docling's rules,
//! mirrored here (#386):
//!
//! - the **root** is the first `multipart/related` entity in the message; its
//!   `start` parameter names the root part by `Content-ID`, otherwise the first
//!   child is it, and a `multipart/alternative` root yields its last HTML
//!   alternative. A bare `text/html` message is its own root. Anything else —
//!   no `Content-Type`, no `multipart/related`, an empty one, a `start` that
//!   names nothing or a non-HTML part, an empty page — is a conversion
//!   **failure**, as docling reports it, not an empty document;
//! - **resources** are the image parts of that `multipart/related` scope only
//!   (a sibling scope's parts are invisible), keyed by their `Content-Location`
//!   verbatim, by that location resolved against the archive's base, and by
//!   their normalized `cid:` (lower-cased, angle brackets stripped). The first
//!   part to claim a key keeps it;
//! - the **base** is the root part's `Content-Location`: a remote URL as is, a
//!   protocol-relative `//host/…` as `https:`, and a local spelling (a path, a
//!   `file:` URI, a Windows drive path in either slash form) remapped under
//!   the synthetic `thismessage:/` origin — so `<img src>` values resolve
//!   against it the same way `Content-Location` values do and meet in the
//!   map, without a filesystem lookup;
//! - archive images are embedded only under `fetch_images`, like docling's
//!   `HTMLBackendOptions.fetch_images` (the default leaves every `<img>` a
//!   placeholder, even one the archive carries), and a `data:` URI is decoded
//!   by the same switch.
//!
//! What docling does beyond this and we do not: fall back to its shared image
//! loader (local files next to the archive, remote fetches) for a reference
//! the archive lacks. And what we do that docling refuses: `use_web_browser`
//! pre-renders the extracted page (docling fails MHTML input with
//! `render_page`).
//!
//! [RFC 2557]: https://datatracker.ietf.org/doc/html/rfc2557

use std::collections::HashMap;

use mail_parser::{MessageParser, MessagePart, MimeHeaders, PartType};

use crate::backend::images::{build_picture, from_data_uri, ImageResolver};
use crate::backend::{convert_html, maybe_prerender_html, DeclarativeBackend};
use crate::error::ConversionError;
use crate::source::SourceDocument;
use docling_core::{DoclingDocument, PictureImage};

/// docling's `_MHTML_SYNTHETIC_BASE`: the origin a root with no real URL (or a
/// local one) is placed under, so relative references still resolve.
const SYNTHETIC_BASE: &str = "thismessage:/";

#[derive(Default)]
pub struct MhtmlBackend {
    /// Embed the archive's own image parts (and `data:` URIs) — docling's
    /// `fetch_images`; off, every `<img>` stays a placeholder.
    pub fetch_images: bool,
    /// Pre-render the extracted page HTML in a headless browser first (mirrors
    /// [`crate::DocumentConverter::use_web_browser`]).
    pub use_web_browser: bool,
}

impl DeclarativeBackend for MhtmlBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        // mail-parser is liberal (docling's `BytesParser` never fails either):
        // what it cannot read at all is a message without headers.
        let msg = MessageParser::default()
            .parse(&source.bytes)
            .ok_or_else(|| fail("MHTML input has no MIME Content-Type header."))?;
        let parts = &msg.parts;
        let (scope, root) = find_root(parts)?;
        let html = parts[root]
            .text_contents()
            .filter(|h| !h.trim().is_empty())
            .ok_or_else(|| fail("The MHTML HTML root part is empty."))?;

        let base = resolve_base(parts[root].content_location().map(str::trim));
        let html = maybe_prerender_html(html, self.use_web_browser)?;
        let images = if self.fetch_images {
            collect_images(parts, scope, &base)
        } else {
            HashMap::new()
        };
        Ok(convert_html(
            &source.name,
            &html,
            &ArchiveResolver {
                images,
                base,
                fetch_images: self.fetch_images,
            },
        ))
    }
}

/// docling's `ValueError`s from `_parse_mhtml`, surfaced as a conversion failure.
fn fail(msg: &str) -> ConversionError {
    ConversionError::Parse(format!("mhtml: {msg}"))
}

fn content_type(part: &MessagePart) -> Option<String> {
    let ct = part.content_type()?;
    Some(match ct.subtype() {
        Some(sub) => format!("{}/{}", ct.ctype(), sub).to_ascii_lowercase(),
        None => ct.ctype().to_ascii_lowercase(),
    })
}

fn children(part: &MessagePart) -> Vec<usize> {
    match &part.body {
        PartType::Multipart(ids) => ids.iter().map(|&i| i as usize).collect(),
        _ => Vec::new(),
    }
}

/// Every part index under `root`, root first, in document order — docling's
/// `Message.walk()`.
fn walk(parts: &[MessagePart], root: usize, out: &mut Vec<usize>) {
    out.push(root);
    for c in children(&parts[root]) {
        walk(parts, c, out);
    }
}

/// docling's `_find_mhtml_root`: `(related scope, HTML root part)` indices.
fn find_root(parts: &[MessagePart]) -> Result<(usize, usize), ConversionError> {
    if parts.is_empty() || parts[0].content_type().is_none() {
        return Err(fail("MHTML input has no MIME Content-Type header."));
    }
    let mut all = Vec::new();
    walk(parts, 0, &mut all);
    let Some(scope) = all
        .iter()
        .copied()
        .find(|&i| content_type(&parts[i]).as_deref() == Some("multipart/related"))
    else {
        if content_type(&parts[0]).as_deref() == Some("text/html") {
            return Ok((0, 0));
        }
        return Err(fail("MHTML input has no multipart/related root."));
    };
    let kids = children(&parts[scope]);
    if kids.is_empty() {
        return Err(fail("The MHTML multipart/related root is empty."));
    }
    let start = parts[scope]
        .content_type()
        .and_then(|ct| ct.attribute("start"))
        .map(normalize_cid);
    let entity = match start {
        Some(target) => kids
            .iter()
            .copied()
            .find(|&i: &usize| parts[i].content_id().map(normalize_cid).as_deref() == Some(&target))
            .ok_or_else(|| fail("The MHTML start part was not found."))?,
        None => kids[0],
    };
    let root = find_html_root(parts, entity)
        .ok_or_else(|| fail("The MHTML root entity has no HTML representation."))?;
    Ok((scope, root))
}

/// docling's `_find_html_root`: the entity itself when it is HTML, else the
/// last HTML alternative of a `multipart/alternative`.
fn find_html_root(parts: &[MessagePart], entity: usize) -> Option<usize> {
    match content_type(&parts[entity]).as_deref() {
        Some("text/html") => Some(entity),
        Some("multipart/alternative") => children(&parts[entity])
            .iter()
            .rev()
            .find_map(|&c| find_html_root(parts, c)),
        _ => None,
    }
}

/// docling's `_normalize_content_id`: `cid:<id>` lower-cased, whatever
/// spelling (`<…>` brackets, a `cid:` prefix, case) the header or `src` used.
fn normalize_cid(value: &str) -> String {
    let mut id = value.trim();
    if id.len() >= 4 && id[..4].eq_ignore_ascii_case("cid:") {
        id = &id[4..];
    }
    format!("cid:{}", id.trim_matches(['<', '>']).to_lowercase())
}

/// The image parts of the `multipart/related` scope, under every key the page
/// may address them by (docling's `_collect_mhtml_resources`). `None` marks a
/// part whose bytes are not a decodable raster (SVG, an empty payload): the
/// key is still claimed — a later part with the same label does not take it
/// over — and the `<img>` stays a placeholder, as docling warns and skips it.
fn collect_images(
    parts: &[MessagePart],
    scope: usize,
    base: &str,
) -> HashMap<String, Option<PictureImage>> {
    let mut images: HashMap<String, Option<PictureImage>> = HashMap::new();
    let mut ids = Vec::new();
    walk(parts, scope, &mut ids);
    for i in ids {
        let part = &parts[i];
        let Some(ct) = part.content_type() else {
            continue;
        };
        if part.is_multipart() || !ct.ctype().eq_ignore_ascii_case("image") {
            continue;
        }
        let payload = part.contents();
        if payload.is_empty() {
            continue;
        }
        let mimetype = format!("{}/{}", ct.ctype(), ct.subtype().unwrap_or(""));
        let pic = build_picture(mimetype, payload.to_vec());
        if let Some(loc) = part.content_location().map(str::trim) {
            images.entry(loc.to_string()).or_insert_with(|| pic.clone());
            images
                .entry(join_location(base, loc))
                .or_insert_with(|| pic.clone());
        }
        if let Some(id) = part.content_id() {
            images.entry(normalize_cid(id)).or_insert(pic);
        }
    }
    images
}

/// Resolves an `<img src>` the way docling's `_resolve_relative_path` +
/// `_create_image_ref` do with `_mhtml_resources` set: a `cid:` reference by
/// its normalized id, anything else joined onto the archive base and looked
/// up in the map; a `data:` URI decoded inline (docling hands it to its
/// shared loader, gated by the same `fetch_images`).
struct ArchiveResolver {
    images: HashMap<String, Option<PictureImage>>,
    base: String,
    fetch_images: bool,
}

impl ImageResolver for ArchiveResolver {
    fn resolve(&self, src: &str) -> Option<PictureImage> {
        let src = src.trim();
        if src.len() >= 4 && src[..4].eq_ignore_ascii_case("cid:") {
            return self.images.get(&normalize_cid(src)).cloned().flatten();
        }
        if let Some(pic) = self.images.get(&join_location(&self.base, src)) {
            return pic.clone();
        }
        if self.fetch_images && src.starts_with("data:") {
            return from_data_uri(src);
        }
        None
    }
}

/// docling's `_resolve_mhtml_base` with no caller-supplied base (we have no
/// `source_uri` for an archive): the root part's `Content-Location`, remote
/// URLs kept, local spellings remapped under `thismessage:/`.
fn resolve_base(root_location: Option<&str>) -> String {
    let Some(loc) = root_location.filter(|l| !l.is_empty()) else {
        return SYNTHETIC_BASE.to_string();
    };
    if loc.starts_with("//") {
        return format!("https:{loc}");
    }
    if is_remote_url(loc) {
        return loc.to_string();
    }
    if let Some(local) = local_path(loc) {
        return join_location(SYNTHETIC_BASE, &local);
    }
    if scheme_of(loc).is_some() {
        return loc.to_string();
    }
    join_location(SYNTHETIC_BASE, loc)
}

/// docling's `_join_mhtml_location`: resolve `location` against `base`,
/// handling the synthetic origin, local spellings and real URLs alike.
fn join_location(base: &str, location: &str) -> String {
    let location = location.trim();
    let location_is_local = local_path(location).is_some();
    if (!location_is_local && scheme_of(location).is_some()) || location.starts_with("//") {
        return location.to_string();
    }
    if let Some(base_path) = base.strip_prefix("thismessage:") {
        let base_path = base_path
            .split(['?', '#'])
            .next()
            .unwrap_or("")
            .replace('\\', "/");
        let safe = location.replace('\\', "/");
        let joined = posix_normpath(&posix_join(posix_dirname(&base_path), &safe));
        return format!("thismessage:/{}", joined.trim_start_matches('/'));
    }
    if let (Some(base_local), Some(loc_local)) = (local_path(base), local_path(location)) {
        // Both local: a plain directory join (docling uses `ntpath` for a
        // Windows base, `posixpath` otherwise; either way the two sides of a
        // lookup — `Content-Location` and `<img src>` — go through the same
        // join, so one normalized spelling serves).
        let base_local = base_local.replace('\\', "/");
        let loc_local = loc_local.replace('\\', "/");
        return posix_normpath(&posix_join(posix_dirname(&base_local), &loc_local));
    }
    urljoin(base, location)
}

/// docling's `_mhtml_local_path`: the filesystem spelling of a local path or
/// `file:` URI, `None` for anything with a real scheme or host.
fn local_path(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(scheme) = scheme_of(value) {
        if scheme.eq_ignore_ascii_case("file") {
            let rest = &value[5..];
            let (netloc, path) = match rest.strip_prefix("//") {
                Some(r) => r.split_at(r.find(['/', '?', '#']).unwrap_or(r.len())),
                None => ("", rest),
            };
            let path = path.split(['?', '#']).next().unwrap_or("");
            let mut path = percent_decode(path);
            if !netloc.is_empty() && !netloc.eq_ignore_ascii_case("localhost") {
                path = format!("//{netloc}{path}");
            }
            // `file:///C:/x` → `C:/x`.
            let b = path.as_bytes();
            if b.len() >= 4
                && b[0] == b'/'
                && b[1].is_ascii_alphabetic()
                && b[2] == b':'
                && (b[3] == b'/' || b[3] == b'\\')
            {
                path.remove(0);
            }
            return Some(path);
        }
        // `urlparse` reads a drive letter as a one-letter scheme: local.
        if scheme.len() == 1 && !value[scheme.len() + 1..].starts_with("//") {
            return Some(value.to_string());
        }
        return None;
    }
    if value.starts_with("//") {
        return None;
    }
    Some(value.to_string())
}

/// docling's `ImageResourceLoader.is_remote_url`.
fn is_remote_url(value: &str) -> bool {
    scheme_of(value).is_some_and(|s| {
        ["http", "https", "ftp", "s3", "gs"]
            .iter()
            .any(|k| s.eq_ignore_ascii_case(k))
    })
}

/// The URI scheme of `value` (`urlparse(value).scheme`, non-empty), if any.
fn scheme_of(value: &str) -> Option<&str> {
    let end = value.find(':')?;
    let scheme = &value[..end];
    let mut chars = scheme.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
    .then_some(scheme)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn posix_dirname(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => "",
    }
}

fn posix_join(dir: &str, rel: &str) -> String {
    if rel.starts_with('/') || dir.is_empty() {
        rel.to_string()
    } else if dir.ends_with('/') {
        format!("{dir}{rel}")
    } else {
        format!("{dir}/{rel}")
    }
}

/// `posixpath.normpath`: collapse `.`/`..`/`//`; a relative path may keep
/// leading `..` segments, an absolute one drops them.
fn posix_normpath(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if matches!(out.last(), Some(&s) if s != "..") {
                    out.pop();
                } else if !absolute {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    let joined = out.join("/");
    match (absolute, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

/// `urllib.parse.urljoin` for a hierarchical base (`scheme://host/path`):
/// RFC 3986 §5.2 reference resolution, dot segments removed.
fn urljoin(base: &str, reference: &str) -> String {
    if reference.is_empty() {
        return base.to_string();
    }
    let Some(scheme) = scheme_of(base) else {
        return reference.to_string();
    };
    let after_scheme = &base[scheme.len() + 1..];
    let (authority, rest) = match after_scheme.strip_prefix("//") {
        Some(r) => {
            let end = r.find(['/', '?', '#']).unwrap_or(r.len());
            (Some(&r[..end]), &r[end..])
        }
        None => (None, after_scheme),
    };
    let base_path = rest.split(['?', '#']).next().unwrap_or("");
    let origin = match authority {
        Some(a) => format!("{scheme}://{a}"),
        None => format!("{scheme}:"),
    };
    if let Some(r) = reference.strip_prefix("//") {
        return format!("{scheme}://{r}");
    }
    if reference.starts_with('?') || reference.starts_with('#') {
        return format!("{origin}{base_path}{reference}");
    }
    let (ref_path, tail) = match reference.find(['?', '#']) {
        Some(i) => (&reference[..i], &reference[i..]),
        None => (reference, ""),
    };
    let merged = if ref_path.starts_with('/') {
        ref_path.to_string()
    } else if base_path.is_empty() && authority.is_some() {
        format!("/{ref_path}")
    } else {
        posix_join(posix_dirname(base_path), ref_path)
    };
    let mut path = posix_normpath(&merged);
    if ref_path.ends_with('/') && !path.ends_with('/') {
        path.push('/');
    }
    format!("{origin}{path}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::InputFormat;
    use docling_core::Node;

    fn png(rgb: [u8; 3]) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb(rgb));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    fn red() -> Vec<u8> {
        png([255, 0, 0])
    }

    fn blue() -> Vec<u8> {
        png([0, 0, 255])
    }

    fn b64(bytes: &[u8]) -> String {
        docling_core::base64::encode(bytes)
    }

    /// docling's test `_archive`: a `multipart/related` with an HTML root
    /// carrying a `Content-ID` and a `Content-Location`.
    fn archive_at(html: &str, extra_parts: &str, related_options: &str, root: &str) -> Vec<u8> {
        format!(
            "MIME-Version: 1.0\r\n\
             Content-Type: multipart/related; boundary=\"B\"{related_options}\r\n\r\n\
             --B\r\nContent-Type: text/html; charset=utf-8\r\nContent-ID: <root@example>\r\n\
             Content-Location: {root}\r\n\r\n{html}\r\n{extra_parts}--B--\r\n"
        )
        .into_bytes()
    }

    fn archive(html: &str, extra_parts: &str) -> Vec<u8> {
        archive_at(html, extra_parts, "", "https://example.com/docs/page.html")
    }

    fn png_part(headers: &str, bytes: &[u8]) -> String {
        format!(
            "--B\r\nContent-Type: image/png\r\n{headers}\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n",
            b64(bytes)
        )
    }

    fn convert(bytes: Vec<u8>, fetch_images: bool) -> Result<DoclingDocument, ConversionError> {
        MhtmlBackend {
            fetch_images,
            use_web_browser: false,
        }
        .convert(&SourceDocument::from_bytes(
            "sample.mhtml",
            InputFormat::Mhtml,
            bytes,
        ))
    }

    fn md(bytes: Vec<u8>) -> String {
        convert(bytes, false).unwrap().export_to_markdown()
    }

    fn pictures(doc: &DoclingDocument) -> Vec<Option<&PictureImage>> {
        doc.nodes
            .iter()
            .filter_map(|n| match n {
                Node::Picture { image, .. } => Some(image.as_ref()),
                _ => None,
            })
            .collect()
    }

    fn pixel(img: &PictureImage) -> [u8; 3] {
        let rgb = image::load_from_memory(&img.data).unwrap().to_rgb8();
        rgb.get_pixel(0, 0).0
    }

    #[test]
    fn root_html_preserves_standard_html_semantics() {
        let html = r#"<html><body>
          <h1>Пример</h1>
          <p>Текст with <a href="https://example.com/details">a link</a>.</p>
          <ul><li>One</li><li>Two</li></ul>
          <table><tr><th>Key</th><th>Value</th></tr><tr><td>A</td><td>1</td></tr></table>
        </body></html>"#;
        let doc = convert(archive(html, ""), false).unwrap();
        let markdown = doc.export_to_markdown();
        assert!(markdown.contains("# Пример"), "{markdown}");
        assert!(markdown.contains("[a link](https://example.com/details)"));
        assert!(markdown.contains("- One") && markdown.contains("- Two"));
        let tables: Vec<&docling_core::Table> = doc
            .nodes
            .iter()
            .filter_map(|n| match n {
                Node::Table(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].rows, [["Key", "Value"], ["A", "1"]]);
    }

    #[test]
    fn related_start_selects_root_inside_nested_multipart() {
        let data = b"MIME-Version: 1.0\r\n\
            Content-Type: multipart/mixed; boundary=\"OUT\"\r\n\r\n\
            --OUT\r\nContent-Type: multipart/related; boundary=\"IN\"; start=\"<wanted@example>\"\r\n\r\n\
            --IN\r\nContent-Type: text/html\r\nContent-ID: <wrong@example>\r\n\r\n<p>Wrong root</p>\r\n\
            --IN\r\nContent-Type: text/html\r\nContent-ID: <wanted@example>\r\n\r\n<h1>Selected root</h1>\r\n\
            --IN--\r\n--OUT--\r\n"
            .to_vec();
        let markdown = md(data);
        assert!(markdown.contains("Selected root"));
        assert!(!markdown.contains("Wrong root"));
    }

    #[test]
    fn related_start_selects_html_from_multipart_alternative() {
        let data = b"MIME-Version: 1.0\r\n\
            Content-Type: multipart/related; boundary=\"B\"; start=\"<root@example>\"\r\n\r\n\
            --B\r\nContent-Type: multipart/alternative; boundary=\"A\"\r\nContent-ID: <root@example>\r\n\r\n\
            --A\r\nContent-Type: text/plain\r\n\r\nPlain fallback\r\n\
            --A\r\nContent-Type: text/html\r\n\r\n<h1>HTML alternative</h1>\r\n\
            --A--\r\n--B--\r\n"
            .to_vec();
        let markdown = md(data);
        assert!(markdown.contains("# HTML alternative"), "{markdown}");
        assert!(!markdown.contains("Plain fallback"));
    }

    #[test]
    fn related_without_start_does_not_select_later_html() {
        // The first child is the root, and it is not HTML: a failure, not a
        // fallback to the HTML part behind it.
        let data = b"MIME-Version: 1.0\r\n\
            Content-Type: multipart/related; boundary=\"B\"\r\n\r\n\
            --B\r\nContent-Type: text/plain\r\n\r\nFirst root\r\n\
            --B\r\nContent-Type: text/html\r\n\r\n<h1>Wrong root</h1>\r\n\
            --B--\r\n"
            .to_vec();
        let err = convert(data, false).unwrap_err().to_string();
        assert!(err.contains("no HTML representation"), "{err}");
    }

    #[test]
    fn declared_non_utf8_charset_is_decoded() {
        let html: Vec<u8> = "<html><body><p>Привет, мир</p></body></html>"
            .chars()
            .map(|c| match c {
                'А'..='я' => (0xC0 + (c as u32 - 'А' as u32)) as u8,
                c => c as u8,
            })
            .collect();
        let data = format!(
            "MIME-Version: 1.0\r\nContent-Type: text/html; charset=windows-1251\r\n\
             Content-Transfer-Encoding: base64\r\n\r\n{}\r\n",
            b64(&html)
        )
        .into_bytes();
        assert!(md(data).contains("Привет, мир"));
    }

    #[test]
    fn real_blink_fixture_decodes_quoted_printable_html() {
        // docling's `tests/data/mhtml/sources/example.mhtml`, mirrored (#386).
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/data/mhtml/sources/example.mhtml");
        let bytes = std::fs::read(path).expect("mirrored upstream fixture");
        let markdown = md(bytes);
        assert!(markdown.contains("# Example Domain"), "{markdown}");
        assert!(markdown.contains("[Learn more](https://iana.org/domains/example)"));
    }

    #[test]
    fn embedded_images_resolve_by_location_and_cid() {
        let parts = png_part(
            "Content-Location: https://example.com/images/location.png",
            &red(),
        ) + &png_part("Content-ID: <CID-IMAGE@EXAMPLE>", &red());
        // The relative `src` resolves against the root's Content-Location; the
        // `cid:` matches case-insensitively.
        let html = r#"<html><body><img src="../images/location.png"><img src="cid:cid-image@example"></body></html>"#;
        let doc = convert(archive(html, &parts), true).unwrap();
        let pics = pictures(&doc);
        assert_eq!(pics.len(), 2);
        for p in pics {
            let img = p.expect("embedded");
            assert_eq!((img.width, img.height), (1, 1));
        }
    }

    #[test]
    fn embedded_relative_location_resolves_with_file_root() {
        let parts = png_part("Content-Location: images/file.png", &red());
        let data = archive_at(
            r#"<html><body><img src="images/file.png"></body></html>"#,
            &parts,
            "",
            "file:///C:/saved/page.html",
        );
        let doc = convert(data, true).unwrap();
        assert!(pictures(&doc)[0].is_some());
    }

    #[test]
    fn default_fetch_images_leaves_archive_image_as_placeholder() {
        let parts = png_part("Content-ID: <image@example>", &red());
        let doc = convert(
            archive(
                r#"<html><body><img src="cid:image@example"></body></html>"#,
                &parts,
            ),
            false,
        )
        .unwrap();
        assert_eq!(pictures(&doc), [None]);
    }

    #[test]
    fn resources_are_limited_to_selected_related_scope() {
        let data = format!(
            "MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"OUT\"\r\n\r\n\
             --OUT\r\nContent-Type: multipart/related; boundary=\"ONE\"\r\n\r\n\
             --ONE\r\nContent-Type: text/html\r\n\r\n<html><body><img src=\"cid:shared@example\"></body></html>\r\n\
             --ONE\r\nContent-Type: image/png\r\nContent-ID: <shared@example>\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n\
             --ONE--\r\n\
             --OUT\r\nContent-Type: multipart/related; boundary=\"TWO\"\r\n\r\n\
             --TWO\r\nContent-Type: text/html\r\n\r\n<p>Other scope</p>\r\n\
             --TWO\r\nContent-Type: image/png\r\nContent-ID: <shared@example>\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n\
             --TWO--\r\n--OUT--\r\n",
            b64(&red()),
            b64(&blue())
        )
        .into_bytes();
        let doc = convert(data, true).unwrap();
        assert_eq!(pixel(pictures(&doc)[0].unwrap()), [255, 0, 0]);
    }

    #[test]
    fn duplicate_resource_labels_keep_first_part() {
        let parts = png_part("Content-ID: <same@example>", &red())
            + &png_part("Content-ID: <same@example>", &blue());
        let doc = convert(
            archive(
                r#"<html><body><img src="cid:same@example"></body></html>"#,
                &parts,
            ),
            true,
        )
        .unwrap();
        assert_eq!(pixel(pictures(&doc)[0].unwrap()), [255, 0, 0]);
    }

    #[test]
    fn windows_locations_are_joined_without_urljoin() {
        for root in [r"C:\safe\page.html", "C:/safe/page.html"] {
            let first = join_location(root, "images/a.png");
            let equivalent = join_location(root, "./images/a.png");
            assert_eq!(first, "C:/safe/images/a.png", "{root}");
            assert_eq!(equivalent, first);
        }
    }

    #[test]
    fn equivalent_windows_archive_locations_resolve() {
        for root in [
            r"C:\safe\page.html",
            "C:/safe/page.html",
            "file:///C:/safe/page.html",
        ] {
            let parts = png_part("Content-Location: images/a.png", &red());
            let data = archive_at(
                r#"<html><body><img src="./images/a.png"></body></html>"#,
                &parts,
                "",
                root,
            );
            let doc = convert(data, true).unwrap();
            assert_eq!(pixel(pictures(&doc)[0].unwrap()), [255, 0, 0], "{root}");
        }
    }

    #[test]
    fn missing_and_unsupported_images_remain_placeholders() {
        let parts = "--B\r\nContent-Type: image/svg+xml\r\nContent-ID: <vector@example>\r\n\r\n\
                     <svg xmlns=\"http://www.w3.org/2000/svg\"></svg>\r\n\
                     --B\r\nContent-Type: image/png\r\nContent-ID: <empty@example>\r\n\
                     Content-Transfer-Encoding: base64\r\n\r\n\r\n";
        let html = r#"<html><body><img src="cid:missing@example"><img src="cid:vector@example"><img src="cid:empty@example"></body></html>"#;
        let doc = convert(archive(html, parts), true).unwrap();
        assert_eq!(pictures(&doc), [None, None, None]);
    }

    #[test]
    fn data_uri_image_is_decoded_under_fetch_images() {
        let html = format!(
            r#"<html><body><img src="data:image/png;base64,{}"></body></html>"#,
            b64(&red())
        );
        let doc = convert(archive(&html, ""), true).unwrap();
        let img = pictures(&doc)[0].expect("decoded");
        assert_eq!((img.width, img.height), (1, 1));
        let doc = convert(archive(&html, ""), false).unwrap();
        assert_eq!(pictures(&doc), [None]);
    }

    #[test]
    fn unusable_input_fails_cleanly() {
        let cases: [(&[u8], &str); 4] = [
            (
                b"MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nplain text\r\n",
                "no multipart/related root",
            ),
            (b"not a mime message", "no MIME Content-Type header"),
            (
                b"MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\n\r\n--B--\r\n",
                "no multipart/related root",
            ),
            (b"", "no MIME Content-Type header"),
        ];
        for (data, expected) in cases {
            let err = convert(data.to_vec(), false).unwrap_err().to_string();
            assert!(err.contains(expected), "{err}");
        }
        // An empty HTML root is a failure too.
        let err = convert(archive("", ""), false).unwrap_err().to_string();
        assert!(err.contains("root part is empty"), "{err}");
    }

    #[test]
    fn invalid_related_start_fails_cleanly() {
        let html = "<p>Fallback must not be selected</p>";
        let err = convert(
            archive_at(
                html,
                "",
                "; start=\"<missing@example>\"",
                "https://example.com/p.html",
            ),
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("start part was not found"), "{err}");
        let err = convert(
            archive_at(
                html,
                "--B\r\nContent-Type: text/plain\r\nContent-ID: <not-html@example>\r\n\r\nnot html\r\n",
                "; start=\"<not-html@example>\"",
                "https://example.com/p.html",
            ),
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("no HTML representation"), "{err}");
    }

    #[test]
    fn location_helpers_follow_docling() {
        assert_eq!(
            normalize_cid(" <CID-Image@Example> "),
            "cid:cid-image@example"
        );
        assert_eq!(normalize_cid("cid:x@y"), "cid:x@y");
        assert_eq!(resolve_base(None), "thismessage:/");
        assert_eq!(resolve_base(Some("//host/p.html")), "https://host/p.html");
        assert_eq!(
            resolve_base(Some("https://example.com/docs/page.html")),
            "https://example.com/docs/page.html"
        );
        assert_eq!(
            resolve_base(Some("file:///C:/saved/page.html")),
            "thismessage:/C:/saved/page.html"
        );
        assert_eq!(resolve_base(Some("page.html")), "thismessage:/page.html");
        assert_eq!(
            join_location("thismessage:/C:/saved/page.html", "images\\file.png"),
            "thismessage:/C:/saved/images/file.png"
        );
        assert_eq!(
            join_location("https://example.com/docs/page.html", "../images/a.png"),
            "https://example.com/images/a.png"
        );
        assert_eq!(
            join_location("https://example.com/docs/page.html", "/root.png"),
            "https://example.com/root.png"
        );
        assert_eq!(
            join_location("https://example.com/docs/page.html", "//cdn.example/x.png"),
            "//cdn.example/x.png"
        );
        assert_eq!(
            join_location("thismessage:/page.html", "https://a.b/c.png"),
            "https://a.b/c.png"
        );
        assert_eq!(posix_normpath("/a/b/../c/./d"), "/a/c/d");
        assert_eq!(posix_normpath("../x"), "../x");
    }
}
