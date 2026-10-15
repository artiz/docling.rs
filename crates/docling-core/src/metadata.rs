//! Document metadata (#679): what a file says about itself — an EPUB's
//! Dublin Core, an Office package's core and application properties, an
//! ODF `meta.xml`, a photo's EXIF, an audio file's tags, a PDF's Info
//! dictionary — read into [`DocumentMetadata`] on the document, and written
//! out on request as a `metadata` object in the JSON or as YAML front
//! matter at the top of the Markdown ([`MetadataOutput`]). Nothing is
//! written by default, so every export stays byte for byte what it was.
//!
//! The common fields are the ones every source has a name for (a title,
//! authors, a subject, keywords, dates, the tool that wrote the file); what
//! only one kind of file carries keeps its own names in a section of its
//! own: `exif` (the tag names as exiftool prints them: `DateTimeOriginal`,
//! `Make`, `Model`, …), `audio` (`album`, `artist`, `genre`, `duration`,
//! …), `properties` (the rest of an Office package's properties:
//! `revision`, `category`, `pages`, …) and a decoded `gps` position.
//!
//! Under the PII pass (`redact_pii`) the author fields are masked as
//! persons and the position is dropped; see [`crate::redact`].

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// Where the metadata goes in the exports. `None` (the default) keeps it to
/// the in-memory document; `Json` adds a `metadata` object to the docling
/// JSON (after `origin`); `Frontmatter` opens the Markdown with a YAML front
/// matter block as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MetadataOutput {
    #[default]
    None,
    Json,
    Frontmatter,
}

impl MetadataOutput {
    /// The option's accepted spellings, for error messages.
    pub const ACCEPTED: &'static str = "none, json, frontmatter";

    /// Parse the wire spelling (case-insensitive; `front_matter` and
    /// `front-matter` read as `frontmatter`).
    pub fn parse(s: &str) -> Option<Self> {
        match s
            .trim()
            .to_ascii_lowercase()
            .replace(['_', '-'], "")
            .as_str()
        {
            "none" => Some(Self::None),
            "json" => Some(Self::Json),
            "frontmatter" => Some(Self::Frontmatter),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Json => "json",
            Self::Frontmatter => "frontmatter",
        }
    }

    /// Whether the JSON export carries the `metadata` object.
    pub fn in_json(self) -> bool {
        self != Self::None
    }

    /// Whether the Markdown export opens with the front matter.
    pub fn front_matter(self) -> bool {
        self == Self::Frontmatter
    }
}

/// A decoded GPS position (EXIF `GPSLatitude` / `GPSLongitude` /
/// `GPSAltitude` with their reference hemispheres applied): signed decimal
/// degrees, metres above sea level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpsPosition {
    pub latitude: f64,
    pub longitude: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub altitude: Option<f64>,
}

/// The metadata of a document. Every field is optional; a document whose
/// file carries nothing has the default, [`is_empty`](Self::is_empty).
/// Dates are kept as the file wrote them, normalized to ISO 8601 where the
/// format defines one (Office and ODF `dcterms` dates, a PDF's `D:` dates);
/// EXIF's own `YYYY:MM:DD HH:MM:SS` stays as it is in `exif` and is turned
/// into ISO 8601 for `date`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DocumentMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rights: Option<String>,
    /// The document's own date: an EPUB's `dc:date`, a photo's
    /// `DateTimeOriginal`, an audio file's `date`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified_by: Option<String>,
    /// The application that wrote the file (Office's `Application`, ODF's
    /// `meta:generator`, EXIF's `Software`, a PDF's `Producer`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator_tool: Option<String>,
    /// The source's other named properties, by a lower-case name
    /// (`revision`, `category`, `content_status`, `company`, `pages`,
    /// `words`, `image_size`, …).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gps: Option<GpsPosition>,
    /// EXIF tags by their exiftool name, each value as it displays
    /// (`Make`, `Model`, `DateTimeOriginal`, `ExposureTime`, `GPSLatitude`,
    /// …); binary tags (`MakerNote`, the thumbnail) are left out.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub exif: BTreeMap<String, String>,
    /// An audio file's tags and stream facts by a lower-case name
    /// (`title`, `artist`, `album`, `genre`, `track`, `date`, `duration`,
    /// `sample_rate`, `channels`, `codec`, …).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub audio: BTreeMap<String, String>,
}

impl DocumentMetadata {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The JSON object the docling JSON export carries as `metadata`
    /// (empty fields left out).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// Read back what [`to_json`](Self::to_json) wrote (unknown keys are
    /// ignored, missing ones default).
    pub fn from_json(value: &serde_json::Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }

    /// The YAML front matter block — `---`, one line per present field in
    /// a fixed order, `---` — or an empty string for empty metadata.
    /// Scalars print plain when YAML would read them back as the same
    /// string (`authors: [Test Author]`) and double-quoted otherwise (a
    /// date, a number, a colon, a leading symbol), so a front-matter parser
    /// gets the strings the file held.
    pub fn to_front_matter(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut out = String::from("---\n");
        fn scalar(out: &mut String, key: &str, value: &Option<String>) {
            if let Some(v) = value {
                let _ = writeln!(out, "{}: {}", yaml_key(key), yaml_scalar(v));
            }
        }
        scalar(&mut out, "title", &self.title);
        yaml_list(&mut out, "authors", &self.authors);
        scalar(&mut out, "subject", &self.subject);
        scalar(&mut out, "description", &self.description);
        yaml_list(&mut out, "keywords", &self.keywords);
        scalar(&mut out, "language", &self.language);
        scalar(&mut out, "publisher", &self.publisher);
        scalar(&mut out, "identifier", &self.identifier);
        scalar(&mut out, "rights", &self.rights);
        scalar(&mut out, "date", &self.date);
        scalar(&mut out, "created", &self.created);
        scalar(&mut out, "modified", &self.modified);
        scalar(&mut out, "last_modified_by", &self.last_modified_by);
        scalar(&mut out, "creator_tool", &self.creator_tool);
        yaml_map(&mut out, "properties", &self.properties);
        if let Some(gps) = &self.gps {
            let _ = write!(
                out,
                "gps: {{latitude: {}, longitude: {}",
                gps.latitude, gps.longitude
            );
            if let Some(alt) = gps.altitude {
                let _ = write!(out, ", altitude: {alt}");
            }
            out.push_str("}\n");
        }
        yaml_map(&mut out, "exif", &self.exif);
        yaml_map(&mut out, "audio", &self.audio);
        out.push_str("---\n");
        out
    }
}

fn yaml_list(out: &mut String, key: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    // A flow list when every item prints plain and holds no `,`; a block
    // list otherwise (a quoted item may hold anything).
    if items
        .iter()
        .all(|i| yaml_plain(i) && !i.contains([',', '[', ']']))
    {
        let _ = writeln!(out, "{}: [{}]", yaml_key(key), items.join(", "));
    } else {
        let _ = writeln!(out, "{}:", yaml_key(key));
        for i in items {
            let _ = writeln!(out, "  - {}", yaml_scalar(i));
        }
    }
}

fn yaml_map(out: &mut String, key: &str, map: &BTreeMap<String, String>) {
    if map.is_empty() {
        return;
    }
    let _ = writeln!(out, "{}:", yaml_key(key));
    for (k, v) in map {
        let _ = writeln!(out, "  {}: {}", yaml_key(k), yaml_scalar(v));
    }
}

/// A key as YAML reads it back: plain for the usual identifier shapes,
/// quoted for anything else.
fn yaml_key(key: &str) -> String {
    if !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
        && key.starts_with(|c: char| c.is_alphabetic() || c == '_')
    {
        key.to_string()
    } else {
        yaml_quote(key)
    }
}

/// Whether `s` can print as a plain scalar and read back unchanged: it
/// starts with a letter, holds none of YAML's indicators or line breaks,
/// and is not one of the words YAML 1.1 reads as a boolean or null.
fn yaml_plain(s: &str) -> bool {
    let starts_ok = s.starts_with(|c: char| c.is_alphabetic());
    let clean = !s.contains(['\n', '\r', '\t', '"', '\\', '#', '`', '{', '}'])
        && !s.contains(": ")
        && !s.ends_with(':')
        && !s.ends_with(' ');
    let word = s.to_ascii_lowercase();
    let reserved = matches!(
        word.as_str(),
        "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "y" | "n"
    );
    starts_ok && clean && !reserved
}

fn yaml_scalar(s: &str) -> String {
    if yaml_plain(s) {
        s.to_string()
    } else {
        yaml_quote(s)
    }
}

/// A double-quoted YAML scalar: `\` and `"` escaped, control characters
/// as `\n`, `\t` or `\uXXXX`.
fn yaml_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_prints_plain_where_safe_and_quotes_the_rest() {
        let mut m = DocumentMetadata {
            title: Some("Test EPUB Document".into()),
            authors: vec!["Test Author".into()],
            language: Some("en".into()),
            date: Some("2024-12-18".into()),
            description: Some("A line: with a colon, and \"quotes\"".into()),
            ..Default::default()
        };
        m.exif
            .insert("DateTimeOriginal".into(), "2024:01:02 03:04:05".into());
        m.properties.insert("revision".into(), "3".into());
        m.gps = Some(GpsPosition {
            latitude: 48.8566,
            longitude: 2.3522,
            altitude: None,
        });
        assert_eq!(
            m.to_front_matter(),
            "---\n\
             title: Test EPUB Document\n\
             authors: [Test Author]\n\
             description: \"A line: with a colon, and \\\"quotes\\\"\"\n\
             language: en\n\
             date: \"2024-12-18\"\n\
             properties:\n  revision: \"3\"\n\
             gps: {latitude: 48.8566, longitude: 2.3522}\n\
             exif:\n  DateTimeOriginal: \"2024:01:02 03:04:05\"\n\
             ---\n"
        );
    }

    #[test]
    fn lists_with_commas_or_quotes_go_block_style() {
        let m = DocumentMetadata {
            authors: vec!["Smith, John".into(), "Ada".into()],
            keywords: vec!["yes".into()],
            ..Default::default()
        };
        assert_eq!(
            m.to_front_matter(),
            "---\nauthors:\n  - Smith, John\n  - Ada\nkeywords:\n  - \"yes\"\n---\n"
        );
        assert!(DocumentMetadata::default().to_front_matter().is_empty());
        assert!(DocumentMetadata::default().is_empty());
    }

    #[test]
    fn json_round_trips_and_leaves_empty_fields_out() {
        let m = DocumentMetadata {
            title: Some("T".into()),
            authors: vec!["A".into()],
            ..Default::default()
        };
        let j = m.to_json();
        assert_eq!(j, serde_json::json!({"title": "T", "authors": ["A"]}));
        assert_eq!(DocumentMetadata::from_json(&j), m);
        assert_eq!(
            DocumentMetadata::from_json(&serde_json::json!({"title": "T", "unknown": 1})).title,
            Some("T".into())
        );
    }

    #[test]
    fn the_output_mode_parses_its_spellings() {
        assert_eq!(
            MetadataOutput::parse("Front-Matter"),
            Some(MetadataOutput::Frontmatter)
        );
        assert_eq!(MetadataOutput::parse("json"), Some(MetadataOutput::Json));
        assert_eq!(MetadataOutput::parse("none"), Some(MetadataOutput::None));
        assert_eq!(MetadataOutput::parse("yaml"), None);
        assert!(
            MetadataOutput::Frontmatter.in_json() && MetadataOutput::Frontmatter.front_matter()
        );
        assert!(MetadataOutput::Json.in_json() && !MetadataOutput::Json.front_matter());
        assert!(!MetadataOutput::None.in_json());
    }
}
