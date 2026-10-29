//! A paragraph of styled runs, as the legacy word-processor readers of #216
//! collect it (T602, HWP, the Mac formats, Lotus Word Pro): text arrives a
//! piece at a time with the formatting in force, and leaves as one
//! paragraph node — whitespace runs collapsed to one space across the run
//! boundaries and trimmed, adjacent runs of one style merged, the Markdown
//! escaped with `**` / `*` / `~~` around each run's non-blank core, and the
//! structured [`InlineRun`]s beside it for DocLang (which keeps underline
//! and sub/superscript, which Markdown has no marker for).

use docling_core::{inline_paragraph_node, InlineRun, Node, Script};

use crate::backend::markdown::escape_text;

/// The character formatting a run carries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub script: Script,
}

/// A paragraph being collected.
#[derive(Debug, Default)]
pub(crate) struct Para {
    runs: Vec<(String, Style)>,
    /// Whitespace seen since the last non-space char, not yet written.
    space: bool,
}

impl Para {
    pub fn new() -> Self {
        Self::default()
    }

    /// Text in `style`: whitespace (tabs, line breaks, no-break spaces)
    /// collapses to one space between words.
    pub fn push(&mut self, text: &str, style: Style) {
        for c in text.chars() {
            self.push_char(c, style);
        }
    }

    pub fn push_char(&mut self, c: char, style: Style) {
        if c.is_whitespace() || c == '\u{a0}' {
            self.space = true;
            return;
        }
        if c.is_control() {
            return;
        }
        let space = std::mem::take(&mut self.space) && !self.runs.is_empty();
        match self.runs.last_mut() {
            Some((text, s)) if *s == style => {
                if space {
                    text.push(' ');
                }
                text.push(c);
            }
            Some((text, before)) if space => {
                // The space between two styles goes to the plainer side, so
                // a styled run's text is just its words.
                if *before == Style::default() || style != Style::default() {
                    text.push(' ');
                    self.runs.push((c.to_string(), style));
                } else {
                    self.runs.push((format!(" {c}"), style));
                }
            }
            _ => self.runs.push((c.to_string(), style)),
        }
    }

    /// A word break that is not whitespace in the source (a line end).
    pub fn space(&mut self) {
        self.space = true;
    }

    /// The plain text, whitespace collapsed.
    pub fn text(&self) -> String {
        self.runs.iter().map(|(t, _)| t.as_str()).collect()
    }

    /// Whether no text arrived.
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// The Markdown: each run escaped, its non-blank core wrapped in the
    /// style's markers.
    pub fn markdown(&self) -> String {
        // Runs that differ only in what Markdown cannot show (underline,
        // sub/superscript) render as one: `**1.**`, not `**1****.**`.
        let mut groups: Vec<(String, Style)> = Vec::new();
        for (text, s) in &self.runs {
            let shown = Style {
                underline: false,
                script: Script::Baseline,
                ..*s
            };
            match groups.last_mut() {
                Some((t, g)) if *g == shown => t.push_str(text),
                _ => groups.push((text.clone(), shown)),
            }
        }
        let mut md = String::new();
        for (text, s) in &groups {
            let core = text.trim();
            let lead = &text[..text.len() - text.trim_start().len()];
            let trail = &text[lead.len() + core.len()..];
            md.push_str(lead);
            let mut m = escape_text(core);
            if s.bold {
                m = format!("**{m}**");
            }
            if s.italic {
                m = format!("*{m}*");
            }
            if s.strike {
                m = format!("~~{m}~~");
            }
            md.push_str(&m);
            md.push_str(trail);
        }
        md.trim_end().to_string()
    }

    /// The paragraph node; `None` when no text arrived.
    pub fn into_node(self) -> Option<Node> {
        if self.runs.is_empty() {
            return None;
        }
        let md = self.markdown();
        let runs = self
            .runs
            .into_iter()
            .map(|(text, s)| InlineRun {
                text,
                bold: s.bold,
                italic: s.italic,
                underline: s.underline,
                strike: s.strike,
                script: s.script,
                code: false,
                formula: false,
            })
            .collect();
        Some(inline_paragraph_node(md, runs, false))
    }

    /// Take the paragraph, leaving an empty one.
    pub fn take(&mut self) -> Para {
        std::mem::take(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_collapses_across_runs_and_markers_hug_the_text() {
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        let mut p = Para::new();
        p.push("  Hello \t ", Style::default());
        p.push(" bold ", bold);
        p.push("\n world_x ", Style::default());
        assert_eq!(p.text(), "Hello bold world_x");
        let Some(Node::InlineGroup { md_text, runs, .. }) = p.into_node() else {
            panic!()
        };
        assert_eq!(md_text, "Hello **bold** world\\_x");
        assert_eq!(runs.len(), 3);
        assert!(runs[1].bold && runs[1].text == "bold");
        assert!(Para::new().into_node().is_none());
    }
}
