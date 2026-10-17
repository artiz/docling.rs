//! Pluggable backends (#680): input formats added without forking.
//!
//! A [`DeclarativeBackend`] registered on a [`DocumentConverter`] with
//! [`register_backend`](DocumentConverter::register_backend) is consulted
//! before the built-in dispatch. A [`BackendMatcher`] says which sources it
//! claims — by file extension, by media type, or by a check on the bytes —
//! and the priority orders the registered backends among themselves (the
//! highest first, the earliest registration first among equals). Built-in
//! formats are only reached when no registered backend claims the source, so
//! a backend can add a format (`.rtf2`) or take one over (`.ipynb`).
//!
//! An extension the built-in tables do not know only reaches a converter
//! through its own loaders: [`DocumentConverter::load_file`] and
//! [`load_named_bytes`](DocumentConverter::load_named_bytes) resolve a source
//! a registered backend claims as [`InputFormat::Plugin`] where
//! [`SourceDocument::from_file`] would answer "unknown format". A registered
//! backend's document goes through the same post-processing as a built-in
//! one: enrichment, redaction, the converter's serializer settings.
//!
//! MarkItDown's equivalent is `register_converter(converter, priority)` plus
//! plugins found through the `markitdown.plugin` entry point; the Python
//! package adds both on top of this (`docling_rs.register_backend`,
//! `docling_rs.markitdown`). Loading shared libraries at run time is not
//! offered: a backend is compiled into the program that registers it.
//!
//! [`DeclarativeBackend`]: crate::backend::DeclarativeBackend
//! [`DocumentConverter`]: crate::DocumentConverter
//! [`InputFormat::Plugin`]: crate::InputFormat::Plugin
//! [`SourceDocument::from_file`]: crate::SourceDocument::from_file

use std::sync::Arc;

use crate::backend::DeclarativeBackend;

/// A content check: whether these bytes are the backend's format.
pub type SniffFn = dyn Fn(&[u8]) -> bool + Send + Sync;

/// Which sources a registered backend claims. A source matches when its
/// extension is one of [`extensions`](Self::extensions), *or* its media type
/// one of [`media_types`](Self::media_types), *or* the
/// [`sniff`](Self::sniff) check accepts its bytes — any one suffices. An
/// empty matcher claims nothing.
#[derive(Clone, Default)]
pub struct BackendMatcher {
    extensions: Vec<String>,
    media_types: Vec<String>,
    sniff: Option<Arc<SniffFn>>,
}

impl BackendMatcher {
    /// A matcher that claims nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Claim these file extensions (case-insensitive, with or without the
    /// leading dot: `"rtf2"`, `".RTF2"`).
    pub fn extensions<I, S>(mut self, extensions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.extensions.extend(
            extensions
                .into_iter()
                .map(|e| {
                    e.as_ref()
                        .trim()
                        .trim_start_matches('.')
                        .to_ascii_lowercase()
                })
                .filter(|e| !e.is_empty()),
        );
        self
    }

    /// Claim these media types (`application/x-rtf2`; case-insensitive,
    /// parameters such as `; charset=utf-8` ignored on both sides).
    pub fn media_types<I, S>(mut self, media_types: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.media_types.extend(
            media_types
                .into_iter()
                .map(|m| essence(m.as_ref()))
                .filter(|m| !m.is_empty()),
        );
        self
    }

    /// Claim the sources whose bytes `check` accepts — for a format with a
    /// signature, or one that arrives without a usable name. The check runs
    /// on every source the converter loads or converts, before the built-in
    /// formats, so keep it cheap (a look at the first bytes).
    pub fn sniff(mut self, check: impl Fn(&[u8]) -> bool + Send + Sync + 'static) -> Self {
        self.sniff = Some(Arc::new(check));
        self
    }

    /// The claimed extensions, lowercase, without the dot.
    pub fn extension_list(&self) -> &[String] {
        &self.extensions
    }

    /// The claimed media types, lowercase, without parameters.
    pub fn media_type_list(&self) -> &[String] {
        &self.media_types
    }

    /// Whether a content check is set.
    pub fn has_sniff(&self) -> bool {
        self.sniff.is_some()
    }

    /// Whether a source with this extension, media type and content is
    /// claimed.
    pub fn matches(&self, extension: Option<&str>, media_type: Option<&str>, bytes: &[u8]) -> bool {
        if let Some(ext) = extension {
            let ext = ext.trim_start_matches('.');
            if self.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
                return true;
            }
        }
        if let Some(mt) = media_type {
            let mt = essence(mt);
            if !mt.is_empty() && self.media_types.contains(&mt) {
                return true;
            }
        }
        self.sniff.as_ref().is_some_and(|check| check(bytes))
    }
}

impl std::fmt::Debug for BackendMatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackendMatcher")
            .field("extensions", &self.extensions)
            .field("media_types", &self.media_types)
            .field("sniff", &self.sniff.is_some())
            .finish()
    }
}

/// A backend registered on a converter: its name, what it claims, the
/// backend and its priority. [`DocumentConverter::registered_backends`]
/// lists them in the order they are tried.
///
/// [`DocumentConverter::registered_backends`]: crate::DocumentConverter::registered_backends
#[derive(Clone)]
pub struct RegisteredBackend {
    name: String,
    matcher: BackendMatcher,
    backend: Arc<dyn DeclarativeBackend + Send + Sync>,
    priority: i32,
}

impl RegisteredBackend {
    pub(crate) fn new(
        name: String,
        matcher: BackendMatcher,
        backend: Arc<dyn DeclarativeBackend + Send + Sync>,
        priority: i32,
    ) -> Self {
        Self {
            name,
            matcher,
            backend,
            priority,
        }
    }

    /// The name it was registered under (what format listings print and a
    /// conversion error names).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What it claims.
    pub fn matcher(&self) -> &BackendMatcher {
        &self.matcher
    }

    /// Its priority: higher is tried first.
    pub fn priority(&self) -> i32 {
        self.priority
    }

    /// The backend itself.
    pub fn backend(&self) -> &(dyn DeclarativeBackend + Send + Sync) {
        self.backend.as_ref()
    }
}

impl std::fmt::Debug for RegisteredBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisteredBackend")
            .field("name", &self.name)
            .field("matcher", &self.matcher)
            .field("priority", &self.priority)
            .finish()
    }
}

/// A media type's essence: `Text/X-RTF2; charset=utf-8` → `text/x-rtf2`.
fn essence(media_type: &str) -> String {
    media_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matchers_take_extension_media_type_or_content() {
        let m = BackendMatcher::new()
            .extensions([".RTF2", "rtf2x"])
            .media_types(["Application/X-RTF2; charset=utf-8"])
            .sniff(|b| b.starts_with(b"{\\rtf2"));
        assert_eq!(m.extension_list(), ["rtf2", "rtf2x"]);
        assert_eq!(m.media_type_list(), ["application/x-rtf2"]);
        assert!(m.matches(Some("rtf2"), None, b""));
        assert!(m.matches(Some(".RTF2X"), None, b""));
        assert!(m.matches(None, Some("application/x-rtf2;charset=latin1"), b""));
        assert!(m.matches(Some("bin"), None, b"{\\rtf2 hello}"));
        assert!(!m.matches(Some("rtf"), Some("text/plain"), b"{\\rtf1 x}"));
        assert!(!BackendMatcher::new().matches(Some("rtf2"), Some("x/y"), b"anything"));
        assert!(format!("{m:?}").contains("sniff: true"));
    }
}
