//! WebVTT (`.vtt`) backend — a port of docling's `WebVTTDocumentBackend`.
//!
//! Each cue's payload becomes one paragraph per line. Cue-text spans are parsed:
//! `<b>`/`<i>`/`<u>` apply formatting (bold inner, italic outer, underline no
//! marker), while `<v …>` (voice), `<c …>` (class), `<lang …>` and timestamps are
//! transparent (their inner text is kept, the tag dropped). A line's components
//! are each wrapped in their Markdown markers and joined with single spaces —
//! docling-core's inline-group serialization.

use docling_core::{DoclingDocument, Node};

use crate::backend::markdown::escape_text;
use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;

pub struct WebVttBackend;

#[derive(Default, Clone, Copy)]
struct Fmt {
    bold: bool,
    italic: bool,
}

struct Comp {
    text: String,
    fmt: Fmt,
}

impl DeclarativeBackend for WebVttBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        let content = source.text()?.replace("\r\n", "\n").replace('\r', "\n");
        let mut doc = DoclingDocument::new(&source.name);

        // Split into blank-line-separated blocks.
        let mut blocks: Vec<String> = Vec::new();
        let mut cur = String::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                if !cur.is_empty() {
                    blocks.push(std::mem::take(&mut cur));
                }
            } else {
                if !cur.is_empty() {
                    cur.push('\n');
                }
                cur.push_str(line);
            }
        }
        if !cur.is_empty() {
            blocks.push(cur);
        }

        for block in &blocks {
            // Header: "WEBVTT" optionally followed by a title on the same line.
            if let Some(rest) = block.lines().next().and_then(|l| l.strip_prefix("WEBVTT")) {
                let title = rest.trim();
                if !title.is_empty() {
                    doc.push(Node::Heading {
                        level: 1,
                        text: escape_text(title),
                    });
                }
                continue;
            }
            if block.starts_with("NOTE")
                || block.starts_with("STYLE")
                || block.starts_with("REGION")
            {
                continue;
            }
            // Cue: lines up to the "-->" timing are the identifier; the rest is
            // the payload (one paragraph per line).
            let lines: Vec<&str> = block.lines().collect();
            let Some(ti) = lines.iter().position(|l| l.contains("-->")) else {
                continue;
            };
            let payload = lines[ti + 1..].join("\n");
            for para in parse_cue(&payload) {
                let text = para
                    .iter()
                    .map(serialize_comp)
                    .collect::<Vec<_>>()
                    .join(" ");
                if !text.is_empty() {
                    doc.push(Node::Paragraph { text });
                }
            }
        }
        Ok(doc)
    }
}

/// Wrap a component's (escaped) text in its Markdown markers — bold inner,
/// italic outer, so bold+italic collapses to `***…***`. Underline carries none.
fn serialize_comp(c: &Comp) -> String {
    let mut s = escape_text(&c.text);
    if c.fmt.bold {
        s = format!("**{s}**");
    }
    if c.fmt.italic {
        s = format!("*{s}*");
    }
    s
}

/// Parse a cue payload into paragraphs (split on newlines) of formatted
/// components. `<b>/<i>/<u>` open/close formatting (by base tag name, ignoring
/// classes/annotations); other tags and timestamps are transparent.
fn parse_cue(payload: &str) -> Vec<Vec<Comp>> {
    let chars: Vec<char> = payload.chars().collect();
    let mut paras: Vec<Vec<Comp>> = vec![Vec::new()];
    let (mut bold, mut italic) = (0i32, 0i32);
    let mut buf = String::new();
    let mut k = 0;

    let flush = |buf: &mut String, paras: &mut Vec<Vec<Comp>>, bold: i32, italic: i32| {
        if !buf.is_empty() {
            paras.last_mut().unwrap().push(Comp {
                text: std::mem::take(buf),
                fmt: Fmt {
                    bold: bold > 0,
                    italic: italic > 0,
                },
            });
        }
    };

    while k < chars.len() {
        match chars[k] {
            '<' => {
                let mut j = k + 1;
                while j < chars.len() && chars[j] != '>' {
                    j += 1;
                }
                let tag: String = chars[k + 1..j.min(chars.len())].iter().collect();
                k = j + 1;
                // A cue timestamp tag (`<00:00:00.389>`, karaoke timing) is
                // stripped from the text without splitting the run around it
                // (docling-core#744: `the<…> quick<…> brown` is one span).
                if tag.starts_with(|c: char| c.is_ascii_digit()) {
                    continue;
                }
                flush(&mut buf, &mut paras, bold, italic);
                let closing = tag.starts_with('/');
                let base: String = tag
                    .trim_start_matches('/')
                    .chars()
                    .take_while(|c| c.is_ascii_alphabetic())
                    .collect();
                let delta = if closing { -1 } else { 1 };
                match base.as_str() {
                    "b" => bold += delta,
                    "i" => italic += delta,
                    _ => {} // u (no marker), v, c, lang, ruby, rt, timestamps
                }
            }
            '\n' => {
                flush(&mut buf, &mut paras, bold, italic);
                paras.push(Vec::new());
                k += 1;
            }
            ch => {
                buf.push(ch);
                k += 1;
            }
        }
    }
    flush(&mut buf, &mut paras, bold, italic);
    paras
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::InputFormat;

    fn md(vtt: &str) -> String {
        let src = SourceDocument::from_bytes("t", InputFormat::Vtt, vtt.as_bytes().to_vec());
        WebVttBackend.convert(&src).unwrap().export_to_markdown()
    }

    #[test]
    fn strips_voice_and_skips_notes() {
        let out = md("WEBVTT\n\nNOTE hi\n\n00:01.000 --> 00:02.000\n<v Roger>Hello world\n");
        assert_eq!(out.trim(), "Hello world");
    }

    /// docling#4105: a line terminator inside a voice span starts a new
    /// paragraph, and the text after the span's end joins *that* paragraph —
    /// upstream's items are `["Hello", "there", " and afterwards"]`.
    #[test]
    fn text_after_multiline_span_stays_in_reading_order() {
        let out =
            md("WEBVTT\n\n00:00:01.000 --> 00:00:05.000\n<v Bob>Hello\nthere</v> and afterwards\n");
        assert_eq!(out.trim(), "Hello\n\nthere  and afterwards");
    }

    /// docling-core#744: karaoke cue timestamps (`<00:00:00.389>`) are stripped
    /// without splitting the span — one run, `the quick brown`.
    #[test]
    fn cue_timestamp_tags_are_stripped_from_the_run() {
        let out = md("WEBVTT\n\n00:00:00.030 --> 00:00:02.669\nthe<00:00:00.389> quick<00:00:00.750> brown\n");
        assert_eq!(out.trim(), "the quick brown");
    }

    /// docling-core#749 / docling#4157: bare CR and CRLF line terminators are
    /// valid WebVTT — signature, cue separation and payload all parse.
    #[test]
    fn cr_and_crlf_terminators_parse() {
        assert_eq!(
            md("WEBVTT\r\r00:00:00.000 --> 00:00:01.000\rHello world\r").trim(),
            "Hello world"
        );
        assert_eq!(
            md("WEBVTT\r\n\r\n00:00:00.000 --> 00:00:01.000\r\nHello\r\nworld\r\n").trim(),
            "Hello\n\nworld"
        );
    }

    #[test]
    fn nested_spans_serialize_with_inline_join() {
        // bold inside italic → ***x***; lang is transparent; components joined
        // with single spaces (so the un-stripped span spacing is preserved).
        let out = md("WEBVTT\n\n00:01.000 --> 00:02.000\n\
             a <i>b <lang es>c</lang></i> d\n");
        assert_eq!(out.trim(), "a  *b * *c*  d");
    }
}
