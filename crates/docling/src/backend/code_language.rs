//! docling's `utils/code_language.py`: a code block's programming language,
//! one port for every backend that asks (HTML, Markdown, DOCX, AsciiDoc).
//!
//! The blocks are usually a few lines, so an explicit hint (a Markdown fence
//! token, an HTML `language-*` class) is trusted first, and the content commits
//! to a language only on a strong marker: downstream consumers treat
//! `code_language` as authoritative, so a wrong guess is worse than `unknown`.
//! `None` is docling's `CodeLanguageLabel.UNKNOWN`.

use regex::Regex;

/// `_ALIASES`: common tokens that are not a `CodeLanguageLabel` value.
const ALIASES: &[(&str, &str)] = &[
    ("py", "Python"),
    ("python2", "Python"),
    ("python3", "Python"),
    ("golang", "Go"),
    ("js", "JavaScript"),
    ("jsx", "JavaScript"),
    ("node", "JavaScript"),
    ("nodejs", "JavaScript"),
    ("ts", "TypeScript"),
    ("tsx", "TypeScript"),
    ("cpp", "C++"),
    ("cxx", "C++"),
    ("cc", "C++"),
    ("cs", "C#"),
    ("csharp", "C#"),
    ("yml", "YAML"),
    ("mysql", "SQL"),
    ("postgres", "SQL"),
    ("postgresql", "SQL"),
    ("psql", "SQL"),
    ("sqlite", "SQL"),
    ("plsql", "SQL"),
    ("tsql", "SQL"),
    ("sh", "Bash"),
    ("shell", "Bash"),
    ("zsh", "Bash"),
    ("rb", "Ruby"),
    ("rs", "Rust"),
    ("kt", "Kotlin"),
    ("kts", "Kotlin"),
    ("objc", "ObjectiveC"),
    ("objective-c", "ObjectiveC"),
    ("tex", "Latex"),
    ("vb", "VisualBasic"),
    ("vbnet", "VisualBasic"),
    ("htm", "HTML"),
    ("html5", "HTML"),
    ("xhtml", "HTML"),
];

/// `normalize_code_language`: an explicit language token — a fence info
/// string, an HTML class — as a `CodeLanguageLabel` value; one
/// `language-`/`lang-` prefix is dropped first. `None` when the token is
/// empty or unknown.
pub(super) fn normalize_code_language(hint: &str) -> Option<&'static str> {
    let token = hint.trim().to_lowercase();
    let token = ["language-", "lang-"]
        .iter()
        .find_map(|p| token.strip_prefix(p))
        .unwrap_or(&token);
    if token.is_empty() {
        return None;
    }
    match docling_core::code_language_label(token) {
        "unknown" => ALIASES
            .iter()
            .find(|(alias, _)| *alias == token)
            .map(|(_, label)| *label),
        label => Some(label),
    }
}

/// `detect_code_language(text, hint)`: a known `hint` wins, else the
/// content's own high-precision markers; an ambiguous snippet stays `None`.
pub(super) fn detect_code_language(text: &str, hint: Option<&str>) -> Option<String> {
    if let Some(label) = hint.and_then(normalize_code_language) {
        return Some(label.to_string());
    }
    if text.trim().is_empty() {
        return None;
    }
    detect_from_content(text).map(str::to_string)
}

/// `_detect_from_content`.
fn detect_from_content(text: &str) -> Option<&'static str> {
    let head = text.trim_start();
    if let Some(c) = cached_regex!(r"^#![^\n]*?\b(bash|zsh|sh|node|perl|php|python|ruby)[0-9.]*\b")
        .captures(head)
    {
        return Some(match &c[1] {
            "bash" | "zsh" | "sh" => "Bash",
            "node" => "JavaScript",
            "perl" => "Perl",
            "php" => "PHP",
            "python" => "Python",
            _ => "Ruby",
        });
    }
    if cached_regex!(r"<\?php\b").is_match(text) {
        return Some("PHP");
    }
    if cached_regex!(r"(?i)<!doctype\s+html\b|</(html|head|body)>|<(head|body)[\s>]").is_match(text)
    {
        return Some("HTML");
    }
    if cached_regex!(r"(?im)^[ \t]*FROM\s+\S+").is_match(text)
        && cached_regex!(r"(?im)^[ \t]*(RUN|CMD|COPY|ADD|ENTRYPOINT|WORKDIR|ENV|EXPOSE)\b")
            .is_match(text)
    {
        return Some("Dockerfile");
    }
    if text.contains("#include") {
        if cached_regex!(r"\bstd::|\bcout\b|\btemplate\s*<|\bnamespace\b").is_match(text) {
            return Some("C++");
        }
        if cached_regex!(r"\bint\s+main\s*\(|\bprintf\s*\(|\bscanf\s*\(").is_match(text) {
            return Some("C");
        }
    }
    // The JavaScript/TypeScript/Java/C# family shares too many generic
    // keywords to tell apart from a few lines, so each rule keys on a marker
    // distinctive to one language (`_CONTENT_RULES`, in upstream's order).
    let rules: [(&str, &Regex); 5] = [
        (
            "Go",
            cached_regex!(
                r"(?m)^[ \t]*package\s+main\b|\bfunc\s+\(\w+\s+\*?\w+\)|\bfmt\.(Print|Println|Printf)\b"
            ),
        ),
        (
            "Rust",
            cached_regex!(r"\bfn\s+main\s*\(|\blet\s+mut\b|\bprintln!\s*\(|\bfn\s+\w+[^\n]*->"),
        ),
        (
            "Python",
            cached_regex!(
                r"(?m)^[ \t]*def\s+\w+\s*\([^\n]*\)\s*(->[^\n:]+)?:|^[ \t]*elif\b|\b__name__\b|^[ \t]*from\s+\S+\s+import\b"
            ),
        ),
        (
            "Java",
            cached_regex!(
                r"\bimport\s+java\.|\bSystem\.out\.print|\bpublic\s+static\s+void\s+main"
            ),
        ),
        (
            "C#",
            cached_regex!(
                r"\busing\s+System\b|\bConsole\.(Write|WriteLine)\b|\bnamespace\s+[\w.]+\s*[{;]"
            ),
        ),
    ];
    for (label, re) in rules {
        if re.is_match(text) {
            return Some(label);
        }
    }
    if looks_like_sql(text) {
        return Some("SQL");
    }
    if cached_regex!(
        r"\b(readonly|public|private|protected)\s+\w+\s*:\s*\w+|:\s*(string|number|boolean)(\[\])?\s*[;,)=]"
    )
    .is_match(text)
    {
        return Some("TypeScript");
    }
    if cached_regex!(
        r"\bconsole\.log\s*\(|\brequire\s*\(|\bmodule\.exports\b|\bdocument\.(getElementById|querySelector)\b"
    )
    .is_match(text)
    {
        return Some("JavaScript");
    }
    let stripped = text.trim();
    if stripped.starts_with(['{', '['])
        && serde_json::from_str::<serde::de::IgnoredAny>(stripped).is_ok()
    {
        return Some("JSON");
    }
    None
}

/// The SQL rule of `_CONTENT_RULES`. Its `SELECT` branch tempers its gaps
/// with lookaheads (`(?:(?!\bfrom\b)[^\n])*`), which the `regex` crate has
/// not, so that branch is matched step by step here: a `SELECT` opening a
/// line, the first `FROM` after it on that line — or opening the next line
/// when that one has none — then a `WHERE`/`JOIN`/`GROUP BY`/`ORDER BY`/`;`
/// on the `FROM`'s line or opening the line after. The other branches are
/// plain patterns.
fn looks_like_sql(text: &str) -> bool {
    let from = cached_regex!(r"(?i)\bfrom\b");
    let end = cached_regex!(r"(?i)\bwhere\b|\bjoin\b|\bgroup\s+by\b|\border\s+by\b|;");
    let line_end = |at: usize| text[at..].find('\n').map_or(text.len(), |i| at + i);
    // The keyword `re` matched right after the line break ending at `eol`
    // and the next line's blanks: `(?:\n[ \t]*)?` followed by the keyword.
    let opens_next_line = |re: &Regex, eol: usize| -> Option<usize> {
        let rest = text.get(eol + 1..)?;
        let at = eol + 1 + (rest.len() - rest.trim_start_matches([' ', '\t']).len());
        re.find_at(text, at)
            .filter(|m| m.start() == at)
            .map(|m| m.end())
    };
    for select in cached_regex!(r"(?im)^[ \t]*select\b").find_iter(text) {
        let eol = line_end(select.end());
        let after_from = match from.find_at(text, select.end()) {
            Some(m) if m.start() < eol => Some(m.end()),
            _ => opens_next_line(from, eol),
        };
        let Some(after_from) = after_from else {
            continue;
        };
        let eol = line_end(after_from);
        let ends = match end.find_at(text, after_from) {
            Some(m) if m.start() < eol => true,
            _ => opens_next_line(end, eol).is_some(),
        };
        if ends {
            return true;
        }
    }
    cached_regex!(
        r"(?im)^[ \t]*insert\s+into\s+\w+\s*(\(|values\b|select\b)|^[ \t]*update\s+\w+\s+set\b[^\n]*?=|^[ \t]*delete\s+from\s+\w+\s*(\bwhere\b|;)|^[ \t]*create\s+(table|view|index|database)\s+(if\s+not\s+exists\s+)?\w+\s*(\(|as\b)|^[ \t]*alter\s+table\s+\w+\s+(add|drop|modify|alter|rename)\b|^[ \t]*drop\s+(table|view|index|database)\s+(if\s+exists\s+)?\w+\s*;"
    )
    .is_match(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_and_aliases() {
        assert_eq!(normalize_code_language("language-python"), Some("Python"));
        assert_eq!(normalize_code_language("lang-JS"), Some("JavaScript"));
        assert_eq!(normalize_code_language(" c++ "), Some("C++"));
        assert_eq!(normalize_code_language("yml"), Some("YAML"));
        assert_eq!(normalize_code_language("hljs"), None);
        assert_eq!(normalize_code_language(""), None);
        // A known hint wins over the content.
        assert_eq!(
            detect_code_language("SELECT 1;", Some("py")).as_deref(),
            Some("Python")
        );
    }

    #[test]
    fn content_markers() {
        let d = |t: &str| detect_code_language(t, None);
        assert_eq!(
            d("#!/usr/bin/env python3\nprint(1)").as_deref(),
            Some("Python")
        );
        assert_eq!(d("#!/bin/sh\necho hi").as_deref(), Some("Bash"));
        assert_eq!(d("<?php echo 1;").as_deref(), Some("PHP"));
        assert_eq!(d("<!DOCTYPE html><p>x").as_deref(), Some("HTML"));
        assert_eq!(
            d("FROM rust:1\nRUN cargo build").as_deref(),
            Some("Dockerfile")
        );
        assert_eq!(
            d("#include <iostream>\nstd::cout << 1;").as_deref(),
            Some("C++")
        );
        assert_eq!(d("#include <stdio.h>\nint main() {}").as_deref(), Some("C"));
        assert_eq!(d("package main\n").as_deref(), Some("Go"));
        assert_eq!(d("let mut x = 1;").as_deref(), Some("Rust"));
        assert_eq!(
            d("from docling.document_converter import DocumentConverter\n").as_deref(),
            Some("Python")
        );
        assert_eq!(d("System.out.println(1);").as_deref(), Some("Java"));
        assert_eq!(d("Console.WriteLine(1);").as_deref(), Some("C#"));
        assert_eq!(d("const x: number = 1;").as_deref(), Some("TypeScript"));
        assert_eq!(d("console.log(1)").as_deref(), Some("JavaScript"));
        assert_eq!(d(r#"{"a": [1, 2]}"#).as_deref(), Some("JSON"));
        assert_eq!(d("{not json").as_deref(), None);
        assert_eq!(d("pip install docling").as_deref(), None);
        assert_eq!(d("   ").as_deref(), None);
    }

    #[test]
    fn sql_select_follows_the_tempered_gaps() {
        let d = |t: &str| detect_code_language(t, None);
        assert_eq!(d("SELECT a FROM t WHERE x = 1").as_deref(), Some("SQL"));
        assert_eq!(
            d("select a, b\n  from t\n  order by a").as_deref(),
            Some("SQL")
        );
        assert_eq!(d("SELECT a FROM t;").as_deref(), Some("SQL"));
        // A FROM two lines down is out of reach.
        assert_eq!(d("select a\n\nfrom t where x").as_deref(), None);
        // The first FROM is the one the rest follows: the second line opens
        // with another FROM, not a terminator.
        assert_eq!(d("select a from b\n from c where").as_deref(), None);
        // Prose with the words in it.
        assert_eq!(d("Select the file from the menu.").as_deref(), None);
        assert_eq!(d("INSERT INTO t VALUES (1)").as_deref(), Some("SQL"));
        assert_eq!(d("update t set a = 1").as_deref(), Some("SQL"));
        assert_eq!(
            d("CREATE TABLE IF NOT EXISTS t (a int)").as_deref(),
            Some("SQL")
        );
        assert_eq!(d("drop table t;").as_deref(), Some("SQL"));
    }
}
