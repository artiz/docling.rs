//! Jupyter notebooks (`.ipynb`, #673) — the format MarkItDown's
//! `IpynbConverter` reads and docling has no backend for; a notebook is
//! JSON (nbformat 4: `cells`, `metadata`, `nbformat`), so this is the
//! [`DoclingJsonBackend`](super::DoclingJsonBackend)'s sibling for a
//! `.json` that says `"nbformat"`.
//!
//! The notebook is rewritten as one Markdown text and read by the
//! [`MarkdownBackend`](super::MarkdownBackend), so its cells become what
//! the same Markdown would:
//!
//! * a markdown cell is its source verbatim — headings, lists, tables,
//!   images, math, as docling's Markdown backend reads them;
//! * a code cell is one code item in the kernel's language
//!   (`metadata.kernelspec.language`, else `metadata.language_info.name`,
//!   else `python` — nbformat's default), fenced with more backticks than
//!   the longest run in the cell so the cell cannot close its own fence;
//! * a raw cell is a code block with no language;
//! * `metadata.title` becomes the first heading when the notebook does not
//!   open with one (MarkItDown's `title`: the metadata, else the first
//!   `# ` heading — which is already a heading here).
//!
//! Outputs are dropped by default (`ipynb_outputs=none`: the notebook as
//! its author wrote it, MarkItDown's reading). Under `ipynb_outputs=text`
//! each output follows its cell: a `stream` and a `text/plain` result as a
//! plain code block, an `error` as `ename: evalue`, a raster image
//! (`image/png`, `image/jpeg`, `image/gif`, `image/webp`, `image/bmp`) as
//! a picture with the bytes — the data is part of the file, so it is read
//! under the `embedded` image tier, which `none` is raised to for the
//! notebook's own outputs only. A rich result keeps its plain text over
//! its HTML (a DataFrame prints as its table text, not as markup).
//!
//! A UTF-8 BOM in front of the JSON is dropped (editors and Windows
//! tooling prepend one; MarkItDown's own regression). Executing the
//! notebook is a non-goal: nothing runs.

use std::fmt;
use std::str::FromStr;

use docling_core::DoclingDocument;
use serde_json::Value;

use crate::backend::images::{ImagePolicy, ImageSources};
use crate::backend::{DeclarativeBackend, MarkdownBackend};
use crate::error::ConversionError;
use crate::format::InputFormat;
use crate::source::SourceDocument;

/// What of a notebook's cell outputs the document keeps (#673): the
/// `ipynb_outputs` option.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IpynbOutputs {
    /// Drop every output: the notebook as written (the default; MarkItDown's
    /// reading).
    #[default]
    None,
    /// Keep the textual outputs as code blocks and the raster image outputs
    /// as pictures, each after its cell.
    Text,
}

impl IpynbOutputs {
    /// The option's wire spelling: `none` | `text`.
    pub fn as_str(self) -> &'static str {
        match self {
            IpynbOutputs::None => "none",
            IpynbOutputs::Text => "text",
        }
    }
}

impl fmt::Display for IpynbOutputs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for IpynbOutputs {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(IpynbOutputs::None),
            "text" => Ok(IpynbOutputs::Text),
            other => Err(format!(
                "ipynb_outputs must be one of none, text; got {other:?}"
            )),
        }
    }
}

/// The Jupyter notebook backend: see the [module docs](self).
#[derive(Debug, Default)]
pub struct IpynbBackend {
    /// Strict Markdown flavour, as [`MarkdownBackend::strict`].
    pub strict: bool,
    /// The image policy the markdown cells' images resolve under
    /// ([`MarkdownBackend::images`]); the notebook's own image outputs read
    /// under at least the `embedded` tier.
    pub images: ImagePolicy,
    /// Which outputs the document keeps.
    pub outputs: IpynbOutputs,
}

/// Whether JSON text is a Jupyter notebook: an `nbformat` key and a `cells`
/// key — the cheap test a `.json` named by its content takes before the
/// notebook is parsed (MarkItDown accepts an `application/json` stream the
/// same way).
pub fn looks_like_notebook(text: &str) -> bool {
    let head = text.trim_start_matches('\u{feff}').trim_start();
    head.starts_with('{') && text.contains("\"nbformat\"") && text.contains("\"cells\"")
}

impl IpynbBackend {
    /// Convert a notebook — the raw JSON bytes of [`SourceDocument`] — into
    /// a document, by way of the Markdown it reads as.
    pub fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let text = source.text()?;
        let text = text.trim_start_matches('\u{feff}');
        let notebook: Value =
            serde_json::from_str(text).map_err(|e| ConversionError::with_source("ipynb", e))?;
        if !notebook.is_object() {
            return Err(ConversionError::with_source(
                "ipynb",
                "the notebook is not a JSON object",
            ));
        }
        let markdown = notebook_markdown(&notebook, self.outputs);

        let mut md_source = SourceDocument::from_bytes(
            format!("{}.md", source.name),
            InputFormat::Md,
            markdown.into_bytes(),
        );
        // A markdown cell's `![](figure.png)` resolves next to the notebook
        // under the `local` tier, as it would in a `.md` beside it.
        md_source.path = source.path.clone();

        let mut images = self.images.clone();
        if self.outputs == IpynbOutputs::Text && images.sources == ImageSources::None {
            // The image outputs are bytes of the notebook itself: no
            // filesystem or network is reached by reading them.
            images.sources = ImageSources::Embedded;
        }
        MarkdownBackend {
            strict: self.strict,
            images,
        }
        .convert(&md_source)
    }
}

/// The Markdown a notebook reads as (the module docs' rules), one block
/// per cell (and per kept output), blank-line separated.
pub fn notebook_markdown(notebook: &Value, outputs: IpynbOutputs) -> String {
    let language = kernel_language(notebook);
    let cells = notebook["cells"].as_array();
    let mut blocks: Vec<String> = Vec::new();

    if let Some(title) = notebook["metadata"]["title"].as_str() {
        let title = title.trim();
        let opens_with_heading = cells
            .and_then(|cells| cells.first())
            .filter(|c| c["cell_type"].as_str() == Some("markdown"))
            .map(|c| cell_source(c).trim_start().starts_with('#'))
            .unwrap_or(false);
        if !title.is_empty() && !opens_with_heading {
            blocks.push(format!("# {title}"));
        }
    }

    for cell in cells.into_iter().flatten() {
        let source = cell_source(cell);
        match cell["cell_type"].as_str().unwrap_or("") {
            "markdown" => {
                if !source.trim().is_empty() {
                    blocks.push(source);
                }
            }
            "code" => {
                if !source.trim().is_empty() {
                    blocks.push(fenced(&source, Some(language)));
                }
                if outputs == IpynbOutputs::Text {
                    for output in cell["outputs"].as_array().into_iter().flatten() {
                        blocks.extend(output_block(output));
                    }
                }
            }
            "raw" if !source.trim().is_empty() => blocks.push(fenced(&source, None)),
            _ => {}
        }
    }
    let mut out = blocks.join("\n\n");
    out.push('\n');
    out
}

/// The code cells' language: the kernelspec's, else `language_info`'s,
/// else nbformat's default.
fn kernel_language(notebook: &Value) -> &str {
    let meta = &notebook["metadata"];
    meta["kernelspec"]["language"]
        .as_str()
        .or_else(|| meta["language_info"]["name"].as_str())
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .unwrap_or("python")
}

/// A cell's `source` (nbformat allows a string or a list of lines) as one
/// string without a trailing newline.
fn cell_source(cell: &Value) -> String {
    let text = multiline(&cell["source"]);
    text.trim_end_matches(['\n', '\r']).to_string()
}

/// nbformat's multiline string: a string, or a list of strings concatenated.
fn multiline(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

/// `text` as a fenced block, the fence one backtick longer than the longest
/// run inside it (at least three) so the content cannot close it.
fn fenced(text: &str, language: Option<&str>) -> String {
    let longest = text
        .as_bytes()
        .split(|b| *b != b'`')
        .map(<[u8]>::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}{}\n{text}\n{fence}", language.unwrap_or(""))
}

/// One output's block under `ipynb_outputs=text`, or none for an output
/// with nothing to keep (an HTML-only widget, an unknown type).
fn output_block(output: &Value) -> Option<String> {
    match output["output_type"].as_str().unwrap_or("") {
        "stream" => {
            let text = multiline(&output["text"]);
            let text = text.trim_end_matches(['\n', '\r']);
            (!text.trim().is_empty()).then(|| fenced(text, None))
        }
        "execute_result" | "display_data" => {
            let data = &output["data"];
            for mime in [
                "image/png",
                "image/jpeg",
                "image/gif",
                "image/webp",
                "image/bmp",
            ] {
                if let Some(encoded) = data.get(mime) {
                    let b64: String = multiline(encoded)
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .collect();
                    if !b64.is_empty() {
                        return Some(format!("![output](data:{mime};base64,{b64})"));
                    }
                }
            }
            let text = multiline(&data["text/plain"]);
            let text = text.trim_end_matches(['\n', '\r']);
            (!text.trim().is_empty()).then(|| fenced(text, None))
        }
        "error" => {
            let name = output["ename"].as_str().unwrap_or("").trim();
            let value = output["evalue"].as_str().unwrap_or("").trim();
            let line = match (name.is_empty(), value.is_empty()) {
                (true, true) => return None,
                (false, true) => name.to_string(),
                (true, false) => value.to_string(),
                (false, false) => format!("{name}: {value}"),
            };
            Some(fenced(&line, None))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nb(cells: Value, metadata: Value) -> Value {
        serde_json::json!({
            "nbformat": 4, "nbformat_minor": 5, "metadata": metadata, "cells": cells
        })
    }

    #[test]
    fn cells_become_markdown_fences_and_the_language_follows_the_kernel() {
        let notebook = nb(
            serde_json::json!([
                {"cell_type": "markdown", "source": ["# Title\n", "\n", "Intro."]},
                {"cell_type": "code", "source": "x <- 1\n", "outputs": [
                    {"output_type": "stream", "name": "stdout", "text": ["1\n"]}
                ]},
                {"cell_type": "raw", "source": ["raw ", "text"]},
                {"cell_type": "code", "source": "", "outputs": []}
            ]),
            serde_json::json!({"kernelspec": {"language": "R"}, "language_info": {"name": "python"}}),
        );
        assert_eq!(
            notebook_markdown(&notebook, IpynbOutputs::None),
            "# Title\n\nIntro.\n\n```R\nx <- 1\n```\n\n```\nraw text\n```\n"
        );
        assert_eq!(
            notebook_markdown(&notebook, IpynbOutputs::Text),
            "# Title\n\nIntro.\n\n```R\nx <- 1\n```\n\n```\n1\n```\n\n```\nraw text\n```\n"
        );
    }

    #[test]
    fn the_metadata_title_leads_unless_the_notebook_opens_with_a_heading() {
        let meta = serde_json::json!({"title": "From Metadata"});
        let plain = nb(
            serde_json::json!([{"cell_type": "markdown", "source": "Just text"}]),
            meta.clone(),
        );
        assert!(notebook_markdown(&plain, IpynbOutputs::None)
            .starts_with("# From Metadata\n\nJust text"));
        let headed = nb(
            serde_json::json!([{"cell_type": "markdown", "source": "# Own heading"}]),
            meta,
        );
        assert_eq!(
            notebook_markdown(&headed, IpynbOutputs::None),
            "# Own heading\n"
        );
    }

    #[test]
    fn a_cell_cannot_close_its_own_fence_and_outputs_pick_text_or_image() {
        let notebook = nb(
            serde_json::json!([
                {"cell_type": "code", "source": "s = \"\"\"```\nx\n```\"\"\"", "outputs": [
                    {"output_type": "execute_result", "data": {
                        "text/html": ["<table></table>"], "text/plain": ["  a  b\n", "0 1 2"]}},
                    {"output_type": "display_data", "data": {
                        "image/png": "iVBORw0KGgo=\n", "text/plain": "<Figure>"}},
                    {"output_type": "error", "ename": "ValueError", "evalue": "bad", "traceback": ["..."]},
                    {"output_type": "display_data", "data": {"application/vnd.widget": {}}}
                ]}
            ]),
            serde_json::json!({}),
        );
        let md = notebook_markdown(&notebook, IpynbOutputs::Text);
        assert!(
            md.starts_with("````python\ns = \"\"\"```\nx\n```\"\"\"\n````\n\n"),
            "{md}"
        );
        assert!(md.contains("```\n  a  b\n0 1 2\n```"), "{md}");
        assert!(!md.contains("<table>"), "{md}");
        assert!(
            md.contains("![output](data:image/png;base64,iVBORw0KGgo=)"),
            "{md}"
        );
        assert!(md.ends_with("```\nValueError: bad\n```\n"), "{md}");
    }

    #[test]
    fn option_spellings_round_trip() {
        assert_eq!("none".parse::<IpynbOutputs>(), Ok(IpynbOutputs::None));
        assert_eq!(" Text ".parse::<IpynbOutputs>(), Ok(IpynbOutputs::Text));
        assert!("all"
            .parse::<IpynbOutputs>()
            .unwrap_err()
            .contains("none, text"));
        assert_eq!(IpynbOutputs::Text.to_string(), "text");
        assert!(looks_like_notebook(
            "\u{feff}{\"nbformat\": 4, \"cells\": []}"
        ));
        assert!(!looks_like_notebook(
            "{\"schema_name\": \"DoclingDocument\"}"
        ));
    }
}
