//! Input document loading.
//!
//! The Rust analogue of `docling.datamodel.document.InputDocument`. A
//! `SourceDocument` holds the raw bytes plus a resolved [`InputFormat`]; it is
//! what you hand to [`crate::DocumentConverter::convert`].

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

    /// View the bytes as UTF-8 text, for text-based backends. A leading UTF-8
    /// BOM is dropped (docling#4098/#4109, 2.123.1–2.124): Excel and Google
    /// Sheets write one when exporting "CSV UTF-8", and kept it prefixes the
    /// first cell/line — `# Title`/`= Title` stops being a heading, WebVTT's
    /// signature check misses `WEBVTT`, JSON parsing rejects the document
    /// outright. One strip here covers every text backend.
    pub fn text(&self) -> Result<&str, ConversionError> {
        let text = std::str::from_utf8(&self.bytes)
            .map_err(|e| ConversionError::with_source("input is not valid UTF-8", e))?;
        Ok(text.strip_prefix('\u{feff}').unwrap_or(text))
    }
}
