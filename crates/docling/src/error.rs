//! Error type for conversion.

use std::error::Error as _;
use std::fmt;

pub use docling_core::EncryptionError;

use crate::format::InputFormat;

/// Anything that can go wrong while loading or converting a source document.
#[derive(Debug)]
pub enum ConversionError {
    /// Reading the input from disk failed.
    Io(std::io::Error),
    /// The file extension (or content) did not map to a known format.
    UnknownFormat { hint: String },
    /// The format is known but no backend is wired up for it yet.
    UnsupportedFormat(InputFormat),
    /// The backend recognized the format but failed to parse the content.
    Parse(String),
    /// Loading the input from a URL failed (#668): the SSRF guard refused
    /// it, the transfer failed or overran its bounds, or a `data:` URL is
    /// malformed.
    Fetch(String),
    /// An option the caller set names a reading the input's backend has not
    /// implemented (#851: `track_changes=all` on a `.doc`, `reject` on an
    /// ODT): refused before any output, never answered with another view.
    UnsupportedOption(String),
    /// The requested streaming conversion is not supported (e.g. JSON, or the
    /// referenced image mode, which both need the whole document up front).
    Streaming(String),
    /// The headless-browser pre-render (`--use-web-browser`) failed, or the crate
    /// was built without the `web-browser` feature.
    Browser(String),
    /// A conversion worker panicked — a backend bug reached on some input
    /// (#395/#396). The panic itself is not swallowed: it still unwinds its own
    /// thread and prints its message and backtrace to stderr. This turns it
    /// into an ordinary error for the caller, so a server answers with a 500
    /// instead of a silent empty document and a batch keeps going.
    Panic(String),
    /// The document budget (`DocumentConverter::document_timeout`, #497) ran
    /// out on a **streaming** conversion. Not a failure: every chunk emitted
    /// before it is the partial document, and this is the stream's last item
    /// — the streaming counterpart of [`crate::ConversionStatus::PartialSuccess`]
    /// with a timeout [`crate::ErrorItem`]. Buffered conversions never raise
    /// it; they report the cut on the result.
    Timeout(String),
    /// A dependency failed during conversion. Unlike [`ConversionError::Parse`]
    /// the underlying error is kept alive (not flattened into a string), so
    /// callers can walk [`std::error::Error::source`] and downcast to the
    /// original type — e.g. to tell a truncated archive from malformed XML.
    WithSource {
        /// What was being converted when the failure happened (backend prefix,
        /// mirroring the `Parse` message style: "xlsx", "docling-json", …).
        context: String,
        /// The error that caused the failure, preserved on the chain.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl ConversionError {
    /// Wrap a dependency error, keeping it reachable via
    /// [`std::error::Error::source`] instead of stringifying it.
    pub fn with_source(
        context: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        ConversionError::WithSource {
            context: context.into(),
            source: source.into(),
        }
    }

    /// The error for an encrypted `context` document (#636): displays as
    /// `parse error: <context>: <error>` — the Office wording of #624 — with
    /// the typed [`EncryptionError`] on the source chain for
    /// [`encryption`](Self::encryption).
    pub fn encrypted(context: impl Into<String>, error: EncryptionError) -> Self {
        Self::with_source(context, error)
    }

    /// [`encrypted`](Self::encrypted) with a backend's own wording: `message`
    /// is what the error displays (after `parse error: <context>: `), the
    /// typed value stays the cause. For the readers whose text predates the
    /// type (iWork, WordPerfect) — nothing a user or a test reads changed
    /// when the value arrived.
    pub fn encrypted_with_message(
        context: impl Into<String>,
        error: EncryptionError,
        message: impl Into<String>,
    ) -> Self {
        Self::with_source(
            context,
            EncryptedDocument {
                message: message.into(),
                error,
            },
        )
    }

    /// The typed reason when this error says the document is encrypted
    /// (#636): anywhere on the [`source`](std::error::Error::source) chain —
    /// the PDF reader's [`docling_pdf::PdfError::Encrypted`], the Office
    /// decryptor's value, an iWork or WordPerfect backend's — so a caller
    /// prompts for a password on [`EncryptionError::needs_password`] instead
    /// of matching message text. `None` for every other failure.
    pub fn encryption(&self) -> Option<&EncryptionError> {
        let mut cause: Option<&(dyn std::error::Error + 'static)> = self.source();
        while let Some(c) = cause {
            if let Some(e) = c.downcast_ref::<EncryptionError>() {
                return Some(e);
            }
            cause = c.source();
        }
        None
    }
}

/// An encryption error in a backend's own words; the typed value is its
/// cause. See [`ConversionError::encrypted_with_message`].
#[derive(Debug)]
struct EncryptedDocument {
    message: String,
    error: EncryptionError,
}

impl fmt::Display for EncryptedDocument {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EncryptedDocument {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConversionError::Io(e) => write!(f, "i/o error: {e}"),
            ConversionError::Fetch(msg) => write!(f, "{msg}"),
            ConversionError::UnsupportedOption(msg) => write!(f, "unsupported option: {msg}"),
            ConversionError::UnknownFormat { hint } => {
                write!(f, "could not determine input format (hint: {hint})")
            }
            ConversionError::UnsupportedFormat(fmt) => {
                write!(
                    f,
                    "no backend implemented yet for format '{}'",
                    fmt.as_str()
                )
            }
            ConversionError::Parse(msg) => write!(f, "parse error: {msg}"),
            ConversionError::Streaming(msg) => write!(f, "streaming not supported: {msg}"),
            ConversionError::Browser(msg) => write!(f, "web-browser render error: {msg}"),
            ConversionError::Panic(msg) => write!(f, "conversion panicked: {msg}"),
            ConversionError::Timeout(msg) => write!(f, "document timeout: {msg}"),
            ConversionError::WithSource { context, source } => {
                // A source that already names its format (`PdfError`'s
                // `pdf: …`) is not prefixed twice (`pdf: pdf: …`).
                let msg = source.to_string();
                if msg.starts_with(&format!("{context}: ")) {
                    write!(f, "parse error: {msg}")
                } else {
                    write!(f, "parse error: {context}: {msg}")
                }
            }
        }
    }
}

impl std::error::Error for ConversionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConversionError::Io(e) => Some(e),
            ConversionError::WithSource { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ConversionError {
    fn from(e: std::io::Error) -> Self {
        ConversionError::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn with_source_keeps_the_cause_on_the_chain() {
        let cause = serde_json::from_str::<i32>("boom").unwrap_err();
        let err = ConversionError::with_source("docling-json", cause);

        assert!(err.to_string().starts_with("parse error: docling-json: "));
        let source = err.source().expect("source is chained");
        assert!(
            source.downcast_ref::<serde_json::Error>().is_some(),
            "chained source downcasts to the original type"
        );
    }

    /// A source whose message already names the context is not prefixed
    /// with it again (`parse error: pdf: pdf: …`).
    #[test]
    fn with_source_does_not_repeat_the_context() {
        let cause = std::io::Error::other("pdf: not a readable PDF");
        assert_eq!(
            ConversionError::with_source("pdf", cause).to_string(),
            "parse error: pdf: not a readable PDF"
        );
        let cause = std::io::Error::other("bad header");
        assert_eq!(
            ConversionError::with_source("pdf", cause).to_string(),
            "parse error: pdf: bad header"
        );
    }

    #[test]
    fn stringly_variants_have_no_source() {
        assert!(ConversionError::Parse("x".into()).source().is_none());
    }

    /// #636: the typed value is found however deep it sits — directly as
    /// the source, behind a backend's own wording, or behind another
    /// wrapper (`with_source` stacks) — and its text is the #624 one.
    #[test]
    fn encryption_is_found_anywhere_on_the_chain() {
        let direct = ConversionError::encrypted("docx", EncryptionError::NeedPassword);
        assert_eq!(
            direct.to_string(),
            "parse error: docx: document is encrypted (a password is required to open it)"
        );
        assert_eq!(direct.encryption(), Some(&EncryptionError::NeedPassword));

        let worded = ConversionError::encrypted_with_message(
            "iwork",
            EncryptionError::NotDecryptable("iWork package encryption".into()),
            "the document is password-protected",
        );
        assert_eq!(
            worded.to_string(),
            "parse error: iwork: the document is password-protected"
        );
        assert!(matches!(
            worded.encryption(),
            Some(EncryptionError::NotDecryptable(_))
        ));
        assert!(!worded.encryption().unwrap().needs_password());

        let stacked = ConversionError::with_source("archive entry", worded);
        assert!(stacked.encryption().is_some(), "{stacked}");

        assert_eq!(ConversionError::Parse("bad zip".into()).encryption(), None);
        assert_eq!(
            ConversionError::with_source("xlsx", std::io::Error::other("short read")).encryption(),
            None
        );
    }
}
