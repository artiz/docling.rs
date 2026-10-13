//! OCR standalone pictures the way the picture-OCR enrichment does (#645,
//! #666) — one line per image: `<path>\t<text>`, the text's line breaks as
//! ` | `.
//!
//! `cargo run --release -p docling-pdf --example picture_ocr -- [--scale X]
//! [--orient] <image>...`
//!
//! `--scale` is `picture_ocr_scale`, `--orient` `ocr_textline_orientation`;
//! models resolve as everywhere (`.models/`, `DOCLING_*` overrides).
//! `scripts/conformance/picture_ocr_recall.py` drives it.

use docling_pdf::Pipeline;

fn main() {
    let mut scale = None;
    let mut orient = false;
    let mut paths = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--scale" => scale = args.next().and_then(|v| v.parse::<f32>().ok()),
            "--orient" => orient = true,
            _ => paths.push(a),
        }
    }
    let mut p = Pipeline::new()
        .expect("pipeline")
        .no_ocr(true)
        .no_table_former(true)
        .picture_ocr_scale(scale)
        .ocr_textline_orientation(orient);
    for path in paths {
        let text = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| p.decode_picture(&b).map_err(|e| e.to_string()))
            .and_then(|img| p.ocr_picture(&img).map_err(|e| e.to_string()))
            .map(|t| t.map(|t| t.text).unwrap_or_default())
            .unwrap_or_else(|e| format!("ERROR: {e}"));
        println!("{path}\t{}", text.replace('\n', " | "));
    }
}
