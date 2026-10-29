//! The text layer's docling-parse 7 word-gap rules (#921, `pdf_word_gap`) on
//! letter-spaced text: synthetic one-page PDFs (`Tc` sets the tracking, `TJ`
//! numbers the cursor-move word gaps, no space glyphs) and the reporter's
//! public-domain fixtures in `tests/data/word_gap/`. Pure Rust, no models.

#[path = "support/one_page_pdf.rs"]
mod one_page_pdf;

use docling_pdf::WordGap;

const MODES: [WordGap; 2] = [WordGap::Adaptive, WordGap::Factor(0.2)];

/// Long enough to be a text layer, not a vestigial form.
const BODY: &str = "Der Betriebsrat hat die Belegschaft ueber die geplanten Massnahmen rechtzeitig zu unterrichten.";

fn text_of(pdf: &[u8], gap: WordGap) -> String {
    docling_pdf::convert_text_layer_pages_with(pdf, "t.pdf", None, Some(gap))
        .expect("text-layer conversion")
        .export_to_text()
}

/// `lines` (content-stream text operations, one per line) below a body line.
fn page(lines: &[&str]) -> Vec<u8> {
    let mut content = format!("BT /F1 12 Tf 72 760 Td ({BODY}) Tj ET\n");
    for (i, op) in lines.iter().enumerate() {
        content.push_str(&format!(
            "BT /F1 12 Tf 72 {} Td {op} ET\n",
            700 - 40 * i as i32
        ));
    }
    one_page_pdf::pdf_bytes(content.as_bytes())
}

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/word_gap")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A letter-spaced word is one constant cursor move between its letters;
/// over the flanking advances (`i` and `l` are narrow, `D` wide) the gaps
/// straddle any threshold, and docling-parse 7 cuts it mid-word. The
/// constancy in points keeps it whole, and its wider word gaps — a
/// letter-spaced phrase's short words included — still split.
#[test]
fn letter_spaced_words_stay_whole() {
    let pdf = page(&[
        "2.5 Tc (Dienstklasse) Tj",
        "1.3 Tc [(Zulagen) -250 (f\\374r) -150 (alle) -150 (Dienstklassen)] TJ",
        "1.3 Tc [(Dienstklasse) -500 (II)] TJ",
    ]);
    for gap in MODES {
        let text = text_of(&pdf, gap);
        assert!(text.contains("Dienstklasse\n"), "{gap:?}: {text:?}");
        assert!(
            text.contains("Zulagen für alle Dienstklassen"),
            "{gap:?}: {text:?}"
        );
        assert!(text.contains("Dienstklasse II"), "{gap:?}: {text:?}");
    }
}

/// Justified prose: equal word gaps chain up across one-letter words, but
/// the chain is flanked by abutting letters, which no letter-spaced word
/// is — the words stay apart.
#[test]
fn equal_word_gaps_around_one_letter_words_still_split() {
    let pdf = page(&[
        "[(Peter) -400 (W) -400 (J) -400 (Staar,) -400 (Michele) -400 (Dolfi)] TJ",
        "[(von) -330 (\\247) -330 (16) -330 (Abs) -330 (1) -330 (Z) -330 (3) -330 (UrlG)] TJ",
    ]);
    for gap in MODES {
        let text = text_of(&pdf, gap);
        assert!(
            text.contains("Peter W J Staar, Michele Dolfi"),
            "{gap:?}: {text:?}"
        );
        assert!(
            text.contains("von § 16 Abs 1 Z 3 UrlG"),
            "{gap:?}: {text:?}"
        );
    }
}

/// One character repeated at an even spacing is a row of tokens (an OTSL
/// figure's `C C C C`), not a letter-spaced word.
#[test]
fn a_repeated_character_is_no_word() {
    let pdf = page(&["5 Tc (CCCC) Tj"]);
    for gap in MODES {
        let text = text_of(&pdf, gap);
        assert!(text.contains("C C C C"), "{gap:?}: {text:?}");
    }
}

/// Four or more dots in a row are one leader, however unevenly they are
/// spaced; the words either side stay apart from it.
#[test]
fn leader_dots_are_one_token() {
    let pdf =
        page(&["[(Protokolle) -600 (.) -90 (.) -60 (.) -90 (.) -60 (.) -90 (.) -600 (46)] TJ"]);
    for gap in MODES {
        let text = text_of(&pdf, gap);
        assert!(text.contains("Protokolle ...... 46"), "{gap:?}: {text:?}");
    }
}

/// The reporter's fixtures (#921): letter-spaced headings and a signature
/// block read whole; the page's one cursor-move word gap (`4.|entgegen`)
/// still splits.
#[test]
fn official_documents_read_their_letter_spaced_lines_whole() {
    for gap in MODES {
        let at = text_of(&fixture("bgbl-at-2025-ii-6-p1.pdf"), gap);
        assert!(
            at.contains("\nHeimarbeitstarif für die Be- und Verarbeitung"),
            "{gap:?}: {at}"
        );

        let de = text_of(&fixture("bgbl-de-2026-i-300-p4.pdf"), gap);
        for line in [
            "Der Bundeskanzler",
            "Merz",
            "Die Bundesministerin für Wirtschaft und Energie",
            "Katherina Reiche",
        ] {
            assert!(de.lines().any(|l| l == line), "{gap:?} {line}: {de}");
        }
        assert!(de.contains("4. entgegen Artikel 30"), "{gap:?}: {de}");

        let olg = text_of(&fixture("ris-olg-wien-4r133-24v-p1.pdf"), gap);
        assert!(
            olg.lines().any(|l| l == "Entscheidungsgründe:"),
            "{gap:?}: {olg}"
        );
    }
}
