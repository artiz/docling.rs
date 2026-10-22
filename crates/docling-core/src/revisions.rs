//! Tracked changes (#851): the policy a reader applies to a document's
//! revisions — Pandoc's `--track-changes=accept|reject|all` — and the typed
//! revision a text run carries under `all`.

use std::fmt;
use std::str::FromStr;

/// How tracked changes are read (#851), Pandoc's `--track-changes`.
///
/// `Accept` shows insertions and leaves deletions out — the text as it
/// stands once every change is accepted, what the DOCX and ODT readers give
/// without a policy. `Reject` shows deletions and leaves insertions out.
/// `All` keeps both as typed revisions ([`Revision`]) with their author and
/// date. The reader of a format a mode is not implemented for refuses it
/// (`ConversionError::UnsupportedOption` in docling) rather than answering
/// with another view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrackChanges {
    #[default]
    Accept,
    Reject,
    All,
}

impl TrackChanges {
    /// The option's accepted spellings, for error messages.
    pub const ACCEPTED: &'static str = "accept, reject, all";

    /// Parse the wire spelling (case-insensitive); `None` for any other.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "accept" | "accepted" => Some(Self::Accept),
            "reject" | "rejected" => Some(Self::Reject),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Reject => "reject",
            Self::All => "all",
        }
    }
}

impl fmt::Display for TrackChanges {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TrackChanges {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        Self::parse(s)
            .ok_or_else(|| format!("track_changes must be one of {}; got {s:?}", Self::ACCEPTED))
    }
}

/// What a tracked change did to the run it marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionKind {
    /// Inserted text, or the destination of a tracked move.
    Insertion,
    /// Deleted text, or the source of a tracked move.
    Deletion,
}

impl RevisionKind {
    /// Pandoc's class for the span (`insertion` / `deletion`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Insertion => "insertion",
            Self::Deletion => "deletion",
        }
    }
}

/// A typed revision on a text run (#851), kept under [`TrackChanges::All`]:
/// the Pandoc AST writes it as `Span .insertion` / `.deletion` with
/// `author` and `date` attributes, Pandoc's docx reader's shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    pub kind: RevisionKind,
    /// The reviewer (`w:author`), when the file names one.
    pub author: Option<String>,
    /// The change's timestamp (`w:date`), as written in the file.
    pub date: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spellings() {
        assert_eq!("Accept".parse::<TrackChanges>(), Ok(TrackChanges::Accept));
        assert_eq!(" all ".parse::<TrackChanges>(), Ok(TrackChanges::All));
        assert_eq!(TrackChanges::parse("rejected"), Some(TrackChanges::Reject));
        assert!("keep".parse::<TrackChanges>().is_err());
        assert_eq!(TrackChanges::default().as_str(), "accept");
    }
}
