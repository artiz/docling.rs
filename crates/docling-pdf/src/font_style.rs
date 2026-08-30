//! Read weight and slant out of a PDF font name (docling's
//! `docling/utils/font_style.py`, ported for the heading-hierarchy stage,
//! #302).
//!
//! PDF font names are the only styling metadata the text layer carries
//! (`/Helvetica-Bold`, `/NKDKGK+HelveticaNeueLTPro-Bd`, `/KIDKQO+Times-Italic`).
//! There is no standard for encoding weight and slant in that string — only
//! foundry conventions — so [`parse_font_style`] recognizes the common ones
//! and reports everything else as *unknown* rather than guessing.
//!
//! Two rules keep the parser conservative, because a false "bold" silently
//! rewrites a heading level while a miss only falls back to size-only ranking:
//!
//! 1. **Style words are matched as whole tokens**, after splitting the name on
//!    separators and camel-case boundaries. `Avenir-Book` is a regular weight,
//!    but the family `Bookman` is not.
//! 2. **Abbreviations are only honored as a whole separator-delimited part.**
//!    `-Bd` is bold, but the `TB` in `LinLibertineTB` and the `LT` in
//!    `HelveticaNeueLTPro` are not read as styles — foundry tags glued onto a
//!    family name look exactly like weight abbreviations.

/// Weight of text whose font name says nothing about weight.
pub(crate) const REGULAR_WEIGHT: u16 = 400;

/// Weight and slant read from a font name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FontStyle {
    pub weight: u16,
    pub italic: bool,
    /// `false` when the name carried no recognizable style — an unstyled
    /// family, a foundry-tagged name, or a bare resource key such as `/F1`.
    pub known: bool,
}

impl Default for FontStyle {
    fn default() -> Self {
        FontStyle {
            weight: REGULAR_WEIGHT,
            italic: false,
            known: false,
        }
    }
}

/// Bucket a numeric weight into `0` (light/regular), `1` (medium/semibold),
/// `2` (bold and above). Coarse on purpose: heading levels are derived from
/// the distinct classes present in a document, so a finer scale would split
/// near-identical styles into separate levels.
pub(crate) fn weight_class(weight: u16) -> u8 {
    if weight >= 700 {
        2
    } else if weight >= 500 {
        1
    } else {
        0
    }
}

/// Whole style words (docling's `_WEIGHT_TOKENS`). Deliberately excludes short
/// foundry tags (LT, MT, PS, Std, Pro, Com).
fn weight_token(token: &str) -> Option<u16> {
    Some(match token {
        "thin" | "hairline" => 100,
        "extralight" | "ultralight" => 200,
        "light" => 300,
        // "roman" is upright, as in Times-Roman — not a Roman numeral.
        "book" | "normal" | "plain" | "regular" | "roman" => REGULAR_WEIGHT,
        "medium" => 500,
        "demi" | "demibold" | "semi" | "semibold" => 600,
        "bold" => 700,
        "extrabold" | "ultrabold" => 800,
        "black" | "fat" | "heavy" | "poster" | "ultra" => 900,
        _ => return None,
    })
}

fn italic_token(token: &str) -> bool {
    matches!(
        token,
        "italic" | "ital" | "inclined" | "kursiv" | "oblique" | "slanted"
    )
}

/// Camel-case splitting separates the modifier from the weight ("SemiBold" →
/// semi, bold), so recombine the pairs before looking tokens up individually.
fn modifier_weight(modifier: &str, next: &str) -> Option<u16> {
    Some(match (modifier, next) {
        ("semi" | "demi", "bold") => 600,
        ("semi" | "demi", "light") => 350,
        ("extra" | "ultra", "bold") => 800,
        ("extra" | "ultra", "light") => 200,
        ("extra" | "ultra", "black") => 900,
        ("x", "bold") => 800,
        ("x", "light") => 200,
        _ => return None,
    })
}

/// Abbreviations, honored only when they form a complete separator-delimited
/// part. Values are `(weight, italic)`; `None` leaves that aspect unset.
fn part_abbreviation(part: &str) -> Option<(Option<u16>, Option<bool>)> {
    Some(match part {
        "b" | "bd" => (Some(700), None),
        "bi" | "bdit" => (Some(700), Some(true)),
        "blk" => (Some(900), None),
        "i" | "ita" | "it" | "obl" => (None, Some(true)),
        "lt" => (Some(300), None),
        "md" => (Some(500), None),
        "reg" | "rg" | "rom" => (Some(REGULAR_WEIGHT), None),
        "sb" => (Some(600), None),
        _ => return None,
    })
}

/// Split a name part at camel-case and letter/digit boundaries, lower-cased:
/// `HelveticaNeueLTPro` → `helvetica`, `neue`, `lt`, `pro` (docling's
/// `_TOKENS` regex `[A-Z]+(?![a-z])|[A-Z][a-z]+|[a-z]+|\d+`).
fn camel_tokens(part: &str) -> Vec<String> {
    let chars: Vec<char> = part.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            out.push(chars[start..i].iter().collect::<String>());
        } else if c.is_uppercase() {
            // A run of uppercase not followed by lowercase, or one uppercase
            // plus its lowercase tail.
            let start = i;
            i += 1;
            while i < chars.len() && chars[i].is_uppercase() {
                i += 1;
            }
            // If the run ate into the capital of a following TitleCase word
            // ("LTPro" → LT + Pro), give the last capital back.
            if i < chars.len() && chars[i].is_lowercase() && i - start > 1 {
                i -= 1;
            }
            while i < chars.len() && chars[i].is_lowercase() {
                i += 1;
            }
            out.push(chars[start..i].iter().collect::<String>().to_lowercase());
        } else if c.is_lowercase() {
            let start = i;
            while i < chars.len() && chars[i].is_lowercase() {
                i += 1;
            }
            out.push(chars[start..i].iter().collect::<String>());
        } else {
            i += 1; // non-alphanumeric inside a part (rare) — skip
        }
    }
    out
}

/// Read weight and slant from a PDF font name. Returns a regular, upright
/// style with `known: false` when the name carries no recognizable styling.
pub(crate) fn parse_font_style(font_name: &str) -> FontStyle {
    // Strip the leading `/` and the `ABCDEF+` subset prefix (PDF 32000-1
    // 9.6.4: six uppercase letters and a plus).
    let mut name = font_name.trim_start_matches('/');
    let b = name.as_bytes();
    if b.len() > 7 && b[6] == b'+' && b[..6].iter().all(|c| c.is_ascii_uppercase()) {
        name = &name[7..];
    }
    if name.is_empty() {
        return FontStyle::default();
    }

    let mut weight: Option<u16> = None;
    let mut italic: Option<bool> = None;

    for part in name.split(['-', '_', ',', '+', ' ']) {
        if part.is_empty() {
            continue;
        }
        if let Some((part_weight, part_italic)) = part_abbreviation(&part.to_lowercase()) {
            weight = part_weight.or(weight);
            italic = part_italic.or(italic);
            continue;
        }
        let tokens = camel_tokens(part);
        let mut index = 0;
        while index < tokens.len() {
            let token = tokens[index].as_str();
            if index + 1 < tokens.len() {
                if let Some(combined) = modifier_weight(token, tokens[index + 1].as_str()) {
                    weight = Some(combined);
                    index += 2;
                    continue;
                }
            }
            if let Some(w) = weight_token(token) {
                weight = Some(w);
            } else if italic_token(token) {
                italic = Some(true);
            }
            index += 1;
        }
    }

    if weight.is_none() && italic.is_none() {
        return FontStyle::default();
    }
    FontStyle {
        weight: weight.unwrap_or(REGULAR_WEIGHT),
        italic: italic.unwrap_or(false),
        known: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(name: &str) -> (u16, bool, bool) {
        let s = parse_font_style(name);
        (s.weight, s.italic, s.known)
    }

    #[test]
    fn recognizes_common_conventions() {
        // Docling's own doc examples: separator words, subset prefixes,
        // abbreviations as whole parts.
        assert_eq!(style("/Helvetica-Bold"), (700, false, true));
        assert_eq!(style("/NKDKGK+HelveticaNeueLTPro-Bd"), (700, false, true));
        assert_eq!(style("/KIDKQO+Times-Italic"), (400, true, true));
        assert_eq!(style("Times-BoldItalic"), (700, true, true));
        assert_eq!(style("ArialMT,BoldItalic"), (700, true, true));
        assert_eq!(style("Foo-SemiBold"), (600, false, true));
        assert_eq!(style("Avenir-Book"), (400, false, true));
    }

    #[test]
    fn stays_conservative_on_foundry_tags() {
        // Family words and glued foundry tags must NOT read as styles.
        assert_eq!(style("Bookman"), (400, false, false));
        assert_eq!(style("LinLibertineTB"), (400, false, false));
        assert_eq!(style("HelveticaNeueLTPro"), (400, false, false));
        assert_eq!(style("/F1"), (400, false, false));
        assert_eq!(style(""), (400, false, false));
        // Times-Roman is upright regular, known (roman = upright, not italic).
        assert_eq!(style("Times-Roman"), (400, false, true));
    }

    #[test]
    fn camel_case_splits_modifiers() {
        assert_eq!(style("OpenSansSemiBold"), (600, false, true));
        assert_eq!(style("FiraSansExtraLight"), (200, false, true));
        assert_eq!(style("SourceSerifBlackItalic"), (900, true, true));
    }

    #[test]
    fn weight_classes_are_coarse() {
        assert_eq!(weight_class(300), 0);
        assert_eq!(weight_class(400), 0);
        assert_eq!(weight_class(500), 1);
        assert_eq!(weight_class(600), 1);
        assert_eq!(weight_class(700), 2);
        assert_eq!(weight_class(900), 2);
    }
}
