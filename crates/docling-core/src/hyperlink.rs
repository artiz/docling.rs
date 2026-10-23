//! Hyperlink destinations as the serializers write them (#860; docling-core
//! 2.104.0's `is_safe_hyperlink` / `markdown_link_destination`).
//!
//! A destination comes from the converted document, where any string is a
//! valid `hyperlink` — a DOCX relationship, an HTML `href`, a PDF link
//! annotation. One with an active scheme (`javascript:`, `data:`,
//! `vbscript:`) must not reach a reader as a clickable link, and one with a
//! space or a `)` must not end a Markdown inline link early and leave its
//! tail to be read as Markdown or raw HTML. The backends bake links into an
//! item's text as `[text](url)` (the flat chain), so the sanitizing lives in
//! one helper every bake site and every serializer goes through: an unsafe
//! destination keeps the link text and drops the link; a safe one is written
//! percent-escaped. The JSON export keeps the raw URI, as docling's model
//! does — a consumer of the JSON decides for itself.

/// URI schemes that are written as live hyperlinks; any other scheme keeps
/// only the link text (docling-core's `SAFE_HYPERLINK_SCHEMES`).
pub const SAFE_HYPERLINK_SCHEMES: &[&str] = &[
    "http", "https", "ftp", "ftps", "sftp", "mailto", "tel", "file",
];

/// Whether a hyperlink destination may be rendered as a live link.
///
/// The scheme is read the way browsers do: control characters and spaces
/// ahead of it are ignored and tabs / newlines are removed anywhere, so
/// `" java\nscript:alert(1)"` is `javascript:`. A destination without a
/// scheme is a relative reference, and a one-letter scheme is a Windows
/// drive (`C:\…`) — both are safe.
pub fn is_safe_hyperlink(uri: &str) -> bool {
    let cleaned: String = uri
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .skip_while(|&c| (c as u32) < 0x21)
        .collect();
    let mut chars = cleaned.chars();
    let Some(first) = chars.next().filter(char::is_ascii_alphabetic) else {
        return true;
    };
    let mut scheme = first.to_string();
    for c in chars {
        if c == ':' {
            return scheme.len() == 1
                || SAFE_HYPERLINK_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str());
        }
        if c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-') {
            scheme.push(c);
        } else {
            // Not a scheme at all (`a b:c`, `my/page:1`) — a relative reference.
            return true;
        }
    }
    true
}

/// The destination as it can stand inside `[text](…)`: control characters,
/// whitespace, parentheses, angle brackets, double quotes and backslashes are
/// percent-encoded — each would otherwise end the destination or open raw
/// HTML — and an `&` that starts an entity (`&amp;`, `&#39;`) becomes
/// `&amp;`, since Markdown renderers decode entities in a destination. Every
/// other byte, an existing `%XX` included, is kept as it is.
pub fn markdown_link_destination(uri: &str) -> String {
    let mut out = String::with_capacity(uri.len());
    let bytes = uri.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'&' if starts_entity(&bytes[i + 1..]) => out.push_str("&amp;"),
            0x00..=0x20 | 0x7f | b'(' | b')' | b'<' | b'>' | b'"' | b'\\' => {
                out.push_str(&format!("%{b:02X}"));
            }
            _ => {
                // Copy a whole UTF-8 sequence; only ASCII is ever escaped.
                let len = utf8_len(b);
                out.push_str(&uri[i..i + len]);
                i += len;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// `&` followed by `#?\w+;` — the regex docling-core matches an entity with
/// (`\w` there is Unicode-aware; an entity name is ASCII in practice).
fn starts_entity(rest: &[u8]) -> bool {
    let rest = rest.strip_prefix(b"#").unwrap_or(rest);
    let n = rest
        .iter()
        .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_')
        .count();
    n > 0 && rest.get(n) == Some(&b';')
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

/// A Markdown inline link `[text](destination)` around `text` — or `text`
/// alone when the destination is not [safe](is_safe_hyperlink), the way
/// docling-core's Markdown serializer writes an item's `hyperlink`.
pub fn markdown_link(text: &str, uri: &str) -> String {
    if is_safe_hyperlink(uri) {
        format!("[{text}]({})", markdown_link_destination(uri))
    } else {
        text.to_string()
    }
}

/// The destination escaped for an HTML `href="…"` attribute (Python's
/// `html.escape`: `&`, `<`, `>`, `"`, `'`), for a [safe](is_safe_hyperlink)
/// destination — the caller leaves the `<a>` out for an unsafe one.
pub fn html_href(uri: &str) -> String {
    let mut out = String::with_capacity(uri.len());
    for c in uri.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_allow_list_read_like_a_browser() {
        // (destination, safe) — docling-core 2.104.0's table, plus the
        // browser-style readings the ticket lists.
        let cases: &[(&str, bool)] = &[
            ("https://example.org/a", true),
            ("HTTP://EXAMPLE.ORG", true),
            ("mailto:a@b.c", true),
            ("tel:+1-555", true),
            ("ftp://host/x", true),
            ("sftp://host/x", true),
            ("file:///tmp/x", true),
            ("page.html#frag", true),
            ("#section", true),
            ("../up/there", true),
            ("", true),
            ("C:\\Users\\x\\doc.pdf", true),
            ("c:/x", true),
            ("my page: notes", true),
            ("javascript:alert(1)", false),
            ("JAVASCRIPT:alert(1)", false),
            ("JaVaScRiPt:alert(1)", false),
            (" javascript:alert(1)", false),
            ("\u{0}javascript:alert(1)", false),
            ("\u{1}\u{2} \tjavascript:alert(1)", false),
            ("java\nscript:alert(1)", false),
            ("java\tscript:alert(1)", false),
            ("java\r\nscript:alert(1)", false),
            ("data:text/html;base64,PHNjcmlwdD4=", false),
            ("vbscript:MsgBox", false),
            ("ms-word:ofe|u|https://x", false),
            ("x-custom+1.a:thing", false),
        ];
        for (uri, safe) in cases {
            assert_eq!(is_safe_hyperlink(uri), *safe, "{uri:?}");
        }
    }

    #[test]
    fn destination_escaping() {
        let cases: &[(&str, &str)] = &[
            ("https://x.org/a(b)%20c d", "https://x.org/a%28b%29%20c%20d"),
            ("https://x.org/?a=1&b=2", "https://x.org/?a=1&b=2"),
            (
                "https://x.org/?a=1&amp;b=2",
                "https://x.org/?a=1&amp;amp;b=2",
            ),
            ("https://x.org/&#39;", "https://x.org/&amp;#39;"),
            (
                "https://x.org/<b>\"q\"\\",
                "https://x.org/%3Cb%3E%22q%22%5C",
            ),
            ("https://x.org/\u{7f}\u{1}", "https://x.org/%7F%01"),
            ("https://x.org/ünïcödé/é", "https://x.org/ünïcödé/é"),
            ("Duck_(film)", "Duck_%28film%29"),
        ];
        for (uri, want) in cases {
            assert_eq!(markdown_link_destination(uri), *want, "{uri:?}");
        }
        assert_eq!(
            markdown_link("paren", "https://x.org/a(b)%20c d"),
            "[paren](https://x.org/a%28b%29%20c%20d)"
        );
        assert_eq!(markdown_link("click", "javascript:alert(1)"), "click");
        assert_eq!(markdown_link("click", "#top"), "[click](#top)");
    }

    #[test]
    fn href_escaping() {
        assert_eq!(
            html_href("https://x.org/?a=1&b=\"2\"<'>"),
            "https://x.org/?a=1&amp;b=&quot;2&quot;&lt;&#x27;&gt;"
        );
    }
}
