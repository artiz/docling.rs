//! Conversion result types.

use docling_core::DoclingDocument;

use crate::format::InputFormat;

/// Outcome status of a conversion, mirroring
/// `docling.datamodel.base_models.ConversionStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionStatus {
    Success,
    PartialSuccess,
    Failure,
}

/// One recorded problem of a conversion that still produced a document —
/// docling's `ErrorItem` (`component_type`, `module_name`, `error_message`).
/// A result with any of these is a [`ConversionStatus::PartialSuccess`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorItem {
    /// docling's `DocItemLabel`-free component tag: `"document_backend"`,
    /// `"model"`, `"doc_assembler"`, `"user_input"`.
    pub component_type: String,
    /// The stage that recorded it (`"pipeline"` for the document budget).
    pub module_name: String,
    pub error_message: String,
}

impl ErrorItem {
    /// The document budget ran out (docling's `document_timeout`, #497):
    /// the pipeline stopped between pages and the document holds the pages
    /// processed.
    pub fn timeout(message: impl Into<String>) -> Self {
        ErrorItem {
            component_type: "document_backend".into(),
            module_name: "pipeline".into(),
            error_message: message.into(),
        }
    }
}

/// The result of converting one [`crate::SourceDocument`].
///
/// Mirrors `docling.datamodel.document.ConversionResult`. The converted
/// [`DoclingDocument`] is exposed directly as `document`, matching the target
/// API (`result.document.export_to_markdown()`); `errors` is non-empty
/// exactly when `status` is [`ConversionStatus::PartialSuccess`] — today the
/// one recorded error is a spent document budget.
#[derive(Debug, Clone)]
pub struct ConversionResult {
    pub document: DoclingDocument,
    pub status: ConversionStatus,
    pub input_name: String,
    pub format: InputFormat,
    pub errors: Vec<ErrorItem>,
    /// What the PII redaction pass removed (#621), when
    /// [`DocumentConverter::redact_pii`](crate::DocumentConverter::redact_pii)
    /// ran: counts per label, and the mapping only when asked for.
    pub redaction: Option<docling_core::RedactionReport>,
}
