//! Input document loading.
//!
//! The Rust analogue of `docling.datamodel.document.InputDocument`. A
//! `SourceDocument` holds the raw bytes plus a resolved [`InputFormat`]; it is
//! what you hand to [`crate::DocumentConverter::convert`].

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use crate::error::ConversionError;
use crate::format::InputFormat;

/// A loaded input document: its name, detected format, and raw bytes.
#[derive(Debug, Clone)]
pub struct SourceDocument {
    pub name: String,
    pub format: InputFormat,
    pub bytes: Vec<u8>,
    /// The filesystem path it was loaded from, if any (`from_file`). Used to
    /// resolve relative `<img src>` paths when image fetching is enabled; `None`
    /// for in-memory sources.
    pub path: Option<PathBuf>,
    /// The URL this document was fetched from, if any. Used to resolve
    /// relative / protocol-relative `<img src>` against the page's origin when
    /// image fetching is enabled (an HTML page fetched from the web references
    /// its images by relative path). `None` for local / in-memory sources.
    pub base_url: Option<String>,
}

impl SourceDocument {
    /// Load a document from a filesystem path, detecting the format from the
    /// extension.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConversionError> {
        let path = path.as_ref();
        let ext = path.extension().and_then(|e| e.to_str()).ok_or_else(|| {
            ConversionError::UnknownFormat {
                hint: format!("no extension on {}", path.display()),
            }
        })?;
        let format =
            InputFormat::from_extension(ext).ok_or_else(|| ConversionError::UnknownFormat {
                hint: format!("unrecognized extension '.{ext}'"),
            })?;
        let bytes = std::fs::read(path)?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document")
            .to_string();
        Ok(Self {
            name,
            format,
            bytes,
            path: Some(path.to_path_buf()),
            base_url: None,
        })
    }

    /// Construct directly from in-memory bytes (no disk access).
    pub fn from_bytes(name: impl Into<String>, format: InputFormat, bytes: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            format,
            bytes,
            path: None,
            base_url: None,
        }
    }

    /// Record the URL this document was fetched from (for resolving relative
    /// `<img src>` against the page origin when image fetching is enabled).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// The directory containing the source file, for resolving relative asset
    /// paths. `None` for in-memory sources.
    pub fn base_dir(&self) -> Option<&Path> {
        self.path.as_deref().and_then(Path::parent)
    }

    /// The bytes as text, for text-based backends — docling's `decode_text`
    /// (docling#4202, 2.129). A byte-order mark settles the encoding first
    /// (UTF-32, UTF-8, UTF-16, and is dropped — docling#4098/#4109: Excel and
    /// Google Sheets write one when exporting "CSV UTF-8", and kept it
    /// prefixes the first cell/line, so `# Title` stops being a heading and
    /// WebVTT's signature check misses `WEBVTT`). Without one the text must
    /// be UTF-8; when it is not, the share of non-ASCII bytes that still form
    /// well-formed UTF-8 sequences decides: at 50% or more the file reads as
    /// *damaged* UTF-8 and is an error (decoding it as a code page would turn
    /// the damage into text that looks fine and is not), below that it is the
    /// one code page docling falls back to, windows-1252 — decoded with a
    /// warning, or an error on the five bytes that page leaves undefined
    /// (Python's `cp1252` rejects them where the WHATWG table maps them to
    /// C1 controls). Borrowed when the bytes are already UTF-8.
    pub fn text(&self) -> Result<Cow<'_, str>, ConversionError> {
        decode_text(&self.bytes)
    }
}

/// See [`SourceDocument::text`].
pub(crate) fn decode_text(bytes: &[u8]) -> Result<Cow<'_, str>, ConversionError> {
    // UTF-32 before UTF-16: the UTF-32 LE mark starts with the UTF-16 LE one.
    if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe, 0x00, 0x00]) {
        return decode_utf32(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0x00, 0x00, 0xfe, 0xff]) {
        return decode_utf32(rest, false);
    }
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        // `decode` sniffs and drops the mark itself.
        let (text, _, malformed) = encoding_rs::UTF_16LE.decode(bytes);
        if malformed {
            return Err(ConversionError::Parse(
                "input carries a UTF-16 byte-order mark but is not valid UTF-16".into(),
            ));
        }
        return Ok(Cow::Owned(text.into_owned()));
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(Cow::Borrowed(text.strip_prefix('\u{feff}').unwrap_or(text))),
        Err(utf8_error) => {
            let coverage = utf8_high_byte_coverage(bytes);
            if coverage >= 0.5 {
                return Err(ConversionError::Parse(format!(
                    "input is not valid UTF-8 ({utf8_error}), but {:.0}% of its non-ASCII \
                     bytes still form well-formed UTF-8 sequences, so it reads as damaged \
                     UTF-8 rather than as another encoding; decoding it as windows-1252 \
                     would turn the damage into text that looks fine and is not",
                    coverage * 100.0
                )));
            }
            // Python's cp1252 has no mapping for these five bytes.
            if bytes
                .iter()
                .any(|b| matches!(b, 0x81 | 0x8d | 0x8f | 0x90 | 0x9d))
            {
                return Err(ConversionError::Parse(
                    "input is neither UTF-8 nor windows-1252, and its encoding is not \
                     declared, so it cannot be decoded reliably"
                        .into(),
                ));
            }
            eprintln!(
                "warning: input is not UTF-8; decoded it as windows-1252 — text in any \
                 other single-byte or multi-byte encoding will be wrong"
            );
            let (text, _) = encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes);
            Ok(Cow::Owned(text.into_owned()))
        }
    }
}

/// Python's `utf-32` codec on the bytes after the mark.
fn decode_utf32(rest: &[u8], little_endian: bool) -> Result<Cow<'static, str>, ConversionError> {
    if !rest.len().is_multiple_of(4) {
        return Err(ConversionError::Parse("truncated UTF-32 input".into()));
    }
    rest.chunks_exact(4)
        .map(|c| {
            let b = [c[0], c[1], c[2], c[3]];
            let u = if little_endian {
                u32::from_le_bytes(b)
            } else {
                u32::from_be_bytes(b)
            };
            char::from_u32(u)
                .ok_or_else(|| ConversionError::Parse(format!("invalid UTF-32 code point {u:#x}")))
        })
        .collect::<Result<String, _>>()
        .map(Cow::Owned)
}

/// docling's `_utf8_high_byte_coverage`: the share of the non-ASCII bytes
/// that belong to well-formed UTF-8 sequences. UTF-8 with a few damaged bytes
/// scores near 1; a single-byte legacy encoding near 0 (its high bytes form
/// no sequences); Shift-JIS / GBK around 0.3.
fn utf8_high_byte_coverage(raw: &[u8]) -> f64 {
    let (mut high, mut covered, mut i) = (0usize, 0usize, 0usize);
    while i < raw.len() {
        let lead = raw[i];
        if lead < 0x80 {
            i += 1;
            continue;
        }
        let len = match lead {
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => 0,
        };
        let well_formed = len > 0
            && raw
                .get(i..i + len)
                .is_some_and(|chunk| std::str::from_utf8(chunk).is_ok());
        if well_formed {
            high += len;
            covered += len;
            i += len;
        } else {
            high += 1;
            i += 1;
        }
    }
    if high == 0 {
        0.0
    } else {
        covered as f64 / high as f64
    }
}

#[cfg(test)]
mod decode_tests {
    use super::decode_text;

    /// docling#4202: a BOM settles the encoding (and is dropped); UTF-8 is
    /// borrowed; a legacy single-byte file decodes as windows-1252; damaged
    /// UTF-8 and bytes cp1252 leaves undefined are errors, not guesses.
    #[test]
    fn text_documents_decode_like_docling() {
        assert_eq!(decode_text("caf\u{e9}".as_bytes()).unwrap(), "caf\u{e9}");
        assert!(matches!(
            decode_text(b"plain").unwrap(),
            std::borrow::Cow::Borrowed(_)
        ));
        assert_eq!(decode_text(b"\xef\xbb\xbf# T").unwrap(), "# T");
        // UTF-16 LE / BE and UTF-32 LE with their marks.
        assert_eq!(decode_text(b"\xff\xfeh\x00i\x00").unwrap(), "hi");
        assert_eq!(decode_text(b"\xfe\xff\x00h\x00i").unwrap(), "hi");
        assert_eq!(decode_text(b"\xff\xfe\x00\x00h\x00\x00\x00").unwrap(), "h");
        // windows-1252: no UTF-8 sequence at all among the high bytes.
        assert_eq!(
            decode_text(b"caf\xe9 \x93q\x94").unwrap(),
            "caf\u{e9} \u{201c}q\u{201d}"
        );
        // Mostly valid UTF-8 with one stray byte: damaged, refused.
        let damaged = decode_text(b"caf\xc3\xa9 na\xc3\xafve \xff");
        assert!(damaged.unwrap_err().to_string().contains("damaged UTF-8"));
        // A byte cp1252 does not define.
        assert!(decode_text(b"x\x81y").is_err());
    }
}
