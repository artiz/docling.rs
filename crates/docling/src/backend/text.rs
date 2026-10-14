//! Plain text, source code, generic JSON and generic XML (#671) — the
//! inputs MarkItDown's plain-text converter takes and docling has no backend
//! for. Nothing is parsed: the text is the document.
//!
//! * [`InputFormat::Text`]: a code extension ([`code_language`]) becomes one
//!   code block in that language — `cat main.py | docling-rs --from text`
//!   reads as ```` ```python ````; anything else (a `.log`, a `Makefile`, an
//!   unknown extension whose bytes are text) becomes paragraphs, one per
//!   blank-line-separated block, lines kept.
//! * [`InputFormat::Jsonl`], and a `.json` that is not a `DoclingDocument`:
//!   one `json` code block, the bytes verbatim — so a key or a UUID in the
//!   file is in the output, non-ASCII included, and nothing is reflowed.
//! * A `.xml` matching no dialect docling reads: one `xml` code block, with
//!   a warning naming the file.
//!
//! The text is decoded like every text backend's input ([`SourceDocument::text`]:
//! BOM / UTF-16 / UTF-32 detection, the `encoding` option), so a
//! windows-1251 log reads as text, not mojibake.

use docling_core::{DoclingDocument, Node};

use crate::error::ConversionError;
use crate::source::SourceDocument;

/// The fenced-code language for a source-code extension (lowercase, no
/// dot), as Markdown renderers name it; `None` for an extension that reads
/// as prose (`log`, `txt`) or is unknown.
pub fn code_language(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "py" | "pyi" => "python",
        "rs" => "rust",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" => "typescript",
        "tsx" => "tsx",
        "jsx" => "jsx",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "scala" => "scala",
        "groovy" | "gradle" => "groovy",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => "cpp",
        "cs" => "csharp",
        "go" => "go",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "m" | "mm" => "objectivec",
        "dart" => "dart",
        "lua" => "lua",
        "pl" | "pm" => "perl",
        "r" => "r",
        "jl" => "julia",
        "hs" => "haskell",
        "ex" | "exs" => "elixir",
        "erl" => "erlang",
        "clj" => "clojure",
        "sh" | "bash" | "zsh" => "bash",
        "fish" => "fish",
        "ps1" => "powershell",
        "bat" | "cmd" => "batch",
        "sql" => "sql",
        "css" => "css",
        "scss" => "scss",
        "less" => "less",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "ini" | "cfg" | "conf" | "properties" | "env" => "ini",
        "proto" => "protobuf",
        "graphql" | "gql" => "graphql",
        "dockerfile" => "dockerfile",
        "makefile" | "mk" => "makefile",
        "cmake" => "cmake",
        "diff" | "patch" => "diff",
        "json" | "jsonl" | "ndjson" => "json",
        "xml" => "xml",
        _ => return None,
    })
}

/// Plain text and source code — [`InputFormat::Text`].
pub struct TextBackend;

impl TextBackend {
    pub fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let text = source.text()?;
        let language = source
            .extension
            .as_deref()
            .and_then(code_language)
            // `Dockerfile` / `Makefile` carry their language as the name.
            .or_else(|| code_language(&source.name.to_ascii_lowercase()));
        Ok(match language {
            Some(lang) => code_document(source, lang, &text),
            None => paragraphs_document(source, &text),
        })
    }
}

/// One code block holding `text` — a source file, a JSON or XML document
/// kept verbatim.
pub fn code_document(source: &SourceDocument, language: &str, text: &str) -> DoclingDocument {
    let mut doc = DoclingDocument::new(source.name.clone());
    let text = text.strip_suffix('\n').unwrap_or(text);
    if !text.trim().is_empty() {
        doc.nodes.push(Node::Code {
            language: Some(language.to_string()),
            text: text.to_string(),
            orig: None,
            pretty: None,
        });
    }
    doc
}

/// Prose: one paragraph per blank-line-separated block, its lines joined
/// with newlines (a log keeps one record per line; Markdown prints them as
/// soft breaks).
pub fn paragraphs_document(source: &SourceDocument, text: &str) -> DoclingDocument {
    let mut doc = DoclingDocument::new(source.name.clone());
    let mut block: Vec<&str> = Vec::new();
    let flush = |block: &mut Vec<&str>, doc: &mut DoclingDocument| {
        if !block.is_empty() {
            doc.nodes.push(Node::Paragraph {
                text: block.join("\n"),
            });
            block.clear();
        }
    };
    for line in text.lines() {
        if line.trim().is_empty() {
            flush(&mut block, &mut doc);
        } else {
            block.push(line.trim_end());
        }
    }
    flush(&mut block, &mut doc);
    doc
}

/// A `.jsonl`/`.ndjson`, or a `.json` that is not a `DoclingDocument`: the
/// bytes as one `json` code block.
pub fn convert_json_text(source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
    let text = source.text()?;
    Ok(code_document(source, "json", &text))
}

/// An XML document matching no dialect docling reads: the bytes as one
/// `xml` code block, after one warning naming the file.
pub fn convert_xml_text(source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
    let text = source.text()?;
    eprintln!(
        "warning: {}.{}: XML matching none of the dialects docling reads (JATS, USPTO, XBRL, \
         DocLang) — kept as one xml code block",
        source.name,
        source.extension.as_deref().unwrap_or("xml")
    );
    Ok(code_document(source, "xml", &text))
}

/// Whether a parsed JSON value is a docling `DoclingDocument` — the
/// `schema_name` says so, or there is a `body` to walk. Anything else is
/// generic JSON (#671): MarkItDown's `test.json`, a config, an API dump.
pub fn is_docling_document(root: &serde_json::Value) -> bool {
    root["schema_name"].as_str() == Some("DoclingDocument") || root.get("body").is_some()
}
