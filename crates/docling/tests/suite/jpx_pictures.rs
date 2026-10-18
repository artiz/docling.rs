//! #598: photographs stored as JPEG 2000 (`JPXDecode`) reach the layout
//! model as pictures, not as the mid-gray placeholder the renderer used to
//! draw. On the reporter's NASA pages (public domain, `tests/fixtures/jpx/`,
//! one page each cut from NASA SP-2010-570 vol. 1 and 2) the blank rectangle
//! made the picture box uncertain: it ran over the caption line below, so
//! the caption nested inside the picture as plain `text` and vanished from
//! the Markdown, and two stacked photos separated by a caption fused into
//! one picture. With the image decoded, every caption links to its picture
//! and prints, and the two photos stay two pictures — what docling (and the
//! docling-parse renderer) give on these pages.
//!
//! Needs the layout model only (`--no-ocr`), so it skips on a checkout
//! without `.models/`.

use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn layout_ready() -> bool {
    let m = repo_root().join(".models");
    let ready = ["layout_heron_int8.onnx", "layout_heron.onnx"]
        .iter()
        .any(|f| m.join(f).exists());
    ready && std::env::set_current_dir(repo_root()).is_ok()
}

fn convert(name: &str) -> (serde_json::Value, String) {
    let path = repo_root()
        .join("crates/docling/tests/fixtures/jpx")
        .join(name);
    let doc = DocumentConverter::new()
        .no_ocr(true)
        .convert(SourceDocument::from_file(&path).expect("fixture"))
        .expect("conversion")
        .document;
    let json: serde_json::Value = serde_json::from_str(&doc.export_to_json()).unwrap();
    (json, doc.export_to_markdown())
}

/// `(picture self_ref, its caption texts)` for every picture.
fn pictures_with_captions(json: &serde_json::Value) -> Vec<(String, Vec<String>)> {
    let texts = json["texts"].as_array().unwrap();
    let text_of = |r: &str| -> String {
        let i: usize = r.trim_start_matches("#/texts/").parse().unwrap();
        texts[i]["text"].as_str().unwrap().to_string()
    };
    json["pictures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let caps = p["captions"]
                .as_array()
                .map(|c| {
                    c.iter()
                        .map(|r| text_of(r["$ref"].as_str().unwrap()))
                        .collect()
                })
                .unwrap_or_default();
            (p["self_ref"].as_str().unwrap().to_string(), caps)
        })
        .collect()
}

#[test]
fn a_full_page_photo_keeps_its_caption() {
    if !layout_ready() {
        eprintln!("skipping: layout model not found");
        return;
    }
    for (file, start) in [
        ("nasa-v1-p192.pdf", "Aerodynamic model of NASA's SCAT-15F"),
        ("nasa-v2-p77.pdf", "A lightning strike reveals"),
    ] {
        let (json, md) = convert(file);
        let pics = pictures_with_captions(&json);
        assert_eq!(pics.len(), 1, "{file}: one picture, got {pics:?}");
        assert_eq!(pics[0].1.len(), 1, "{file}: one caption, got {pics:?}");
        assert!(
            pics[0].1[0].starts_with(start),
            "{file}: caption text {:?}",
            pics[0].1[0]
        );
        assert!(
            md.contains(start),
            "{file}: caption missing from the Markdown:\n{md}"
        );
        // No `text` item is left nested in the picture: the caption is the
        // picture's caption, not a child line the Markdown skips.
        let nested_text = json["texts"].as_array().unwrap().iter().any(|t| {
            t["label"] == "text"
                && t["parent"]["$ref"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("#/pictures/")
        });
        assert!(!nested_text, "{file}: a text item is nested in the picture");
    }
}

#[test]
fn two_stacked_photos_stay_two_pictures() {
    if !layout_ready() {
        eprintln!("skipping: layout model not found");
        return;
    }
    let (json, md) = convert("nasa-v2-p962.pdf");
    let pics = pictures_with_captions(&json);
    assert_eq!(pics.len(), 2, "two pictures, got {pics:?}");
    let caps: Vec<&str> = pics
        .iter()
        .flat_map(|(_, c)| c.iter().map(String::as_str))
        .collect();
    assert_eq!(caps.len(), 2, "one caption each, got {pics:?}");
    for start in ["The Tupolev and NASA flightcrews", "The Tu-144LL landing"] {
        assert!(
            caps.iter().any(|c| c.starts_with(start)),
            "caption {start:?} not linked: {pics:?}"
        );
        assert!(
            md.contains(start),
            "{start:?} missing from the Markdown:\n{md}"
        );
    }
}
