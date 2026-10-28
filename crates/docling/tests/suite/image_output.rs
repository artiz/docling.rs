//! e2e for #519 / #520: the picture `dpi` matches the crop's render scale,
//! `images_scale` resamples picture crops, and `generate_page_images` puts
//! each page's render into the JSON page map (docling-core's
//! `PageItem.image`). Needs the layout model (pictures come from it); skips
//! cleanly without `.models/`, like the other ML tests.

use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};
use serde_json::Value;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn ml_stack_ready() -> bool {
    let root = repo_root();
    root.join(".models/layout_heron.onnx").exists() && std::env::set_current_dir(&root).is_ok()
}

fn convert(converter: DocumentConverter) -> Value {
    let path = repo_root().join("tests/data/pdf/sources/picture_classification.pdf");
    let source = SourceDocument::from_file(&path).expect("picture fixture");
    let doc = converter
        .no_ocr(true)
        .convert(source)
        .expect("convert")
        .document;
    serde_json::from_str(&doc.export_to_json()).expect("valid JSON")
}

/// `(image width px, bbox width pt, dpi)` of every picture with an image.
fn pictures(json: &Value) -> Vec<(f64, f64, u64)> {
    json["pictures"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| {
            let img = p.get("image")?;
            let bbox = &p["prov"][0]["bbox"];
            Some((
                img["size"]["width"].as_f64()?,
                bbox["r"].as_f64()? - bbox["l"].as_f64()?,
                img["dpi"].as_u64()?,
            ))
        })
        .collect()
}

#[test]
fn picture_dpi_matches_the_render_scale() {
    if !ml_stack_ready() {
        eprintln!("skipping: models not found");
        return;
    }
    for (converter, scale) in [
        (DocumentConverter::new(), 2.0),
        (DocumentConverter::new().images_scale(1.0), 1.0),
        (DocumentConverter::new().images_scale(1.5), 1.5),
    ] {
        let json = convert(converter);
        let pics = pictures(&json);
        assert!(!pics.is_empty(), "fixture has pictures");
        for (px, pt, dpi) in pics {
            assert_eq!(dpi, (72.0 * scale) as u64, "dpi at scale {scale}");
            // `dpi` maps pixels back to points: px / (dpi / 72) ≈ the bbox.
            assert!(
                (px * 72.0 / dpi as f64 - pt).abs() <= 1.5,
                "{px}px @ {dpi}dpi vs {pt}pt"
            );
        }
        // No page images unless asked for.
        assert!(json["pages"]["1"].get("image").is_none());
    }
}

#[test]
fn page_images_land_in_the_page_map() {
    if !ml_stack_ready() {
        eprintln!("skipping: models not found");
        return;
    }
    for (converter, scale) in [
        (DocumentConverter::new().generate_page_images(true), 2.0),
        (
            DocumentConverter::new()
                .generate_page_images(true)
                .images_scale(0.5),
            0.5,
        ),
    ] {
        let json = convert(converter);
        let page = json["pages"]["1"].as_object().expect("page 1");
        // docling-core's PageItem field order.
        let keys: Vec<&str> = page.keys().map(String::as_str).collect();
        assert_eq!(keys, ["size", "image", "page_no"]);
        let (w, h) = (
            page["size"]["width"].as_f64().unwrap(),
            page["size"]["height"].as_f64().unwrap(),
        );
        let img = &page["image"];
        assert_eq!(img["mimetype"], "image/png");
        assert_eq!(img["dpi"].as_u64(), Some((72.0 * scale) as u64));
        assert_eq!(img["size"]["width"].as_f64(), Some((w * scale).round()));
        assert_eq!(img["size"]["height"].as_f64(), Some((h * scale).round()));
        assert!(img["uri"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
    }
}

/// #896: `keep_picture_images(false)` on a PDF — the buffered document and
/// the streamed Markdown alike lose the pictures' bytes, the picture items
/// stay; both keep them by default.
#[test]
fn keep_picture_images_false_reaches_pdf_pictures() {
    if !ml_stack_ready() {
        eprintln!("skipping: models not found");
        return;
    }
    let kept = convert(DocumentConverter::new());
    let dropped = convert(DocumentConverter::new().keep_picture_images(false));
    let pictures = |json: &Value| json["pictures"].as_array().unwrap().clone();
    assert!(!pictures(&kept).is_empty(), "the fixture has pictures");
    assert!(pictures(&kept).iter().all(|p| p.get("image").is_some()));
    assert_eq!(pictures(&dropped).len(), pictures(&kept).len());
    assert!(pictures(&dropped).iter().all(|p| p.get("image").is_none()));

    let path = repo_root().join("tests/data/pdf/sources/picture_classification.pdf");
    let options = docling::MarkdownExportOptions {
        image_mode: docling::ImageMode::Embedded,
        ..Default::default()
    };
    let stream = |keep: bool| -> String {
        let source = SourceDocument::from_file(&path).expect("picture fixture");
        DocumentConverter::new()
            .no_ocr(true)
            .keep_picture_images(keep)
            .convert_streaming_with_options(source, &options)
            .expect("stream")
            .map(|chunk| chunk.expect("chunk"))
            .collect()
    };
    assert!(stream(true).contains("data:image/png;base64,"));
    let slim = stream(false);
    assert!(!slim.contains("data:image"), "{slim}");
    assert!(slim.contains("<!-- image -->"), "{slim}");
}
