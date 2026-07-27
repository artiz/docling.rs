//! Run **exactly** what a wasm build does with a PDF: `convert_text_layer` —
//! the pure-Rust content-stream parser, no pdfium, no ONNX. Use it to tell a
//! browser "this PDF needs OCR" apart from a genuine scan: the browser falls
//! back to OCR precisely when this produces no nodes, so an empty result here
//! reproduces that decision offline.
//!
//! Note this is *not* what the CLI's `--no-ocr` runs — that goes through
//! pdfium's text extraction, which can read layers this parser cannot.
//!
//! ```bash
//! cargo run -p docling-pdf --no-default-features --example text_layer -- <pdf>
//! ```
//! (`--no-default-features` keeps pdfium/onnxruntime out of the build.)

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: text_layer <pdf>");
            std::process::exit(2);
        }
    };
    let bytes = std::fs::read(&path).expect("read the pdf");
    let name = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());

    match docling_pdf::convert_text_layer(&bytes, &name) {
        Ok(doc) => {
            let md = doc.export_to_markdown();
            eprintln!(
                "nodes: {}, markdown: {} chars — {}",
                doc.nodes.len(),
                md.len(),
                if doc.nodes.is_empty() {
                    "EMPTY: a browser build would fall back to OCR here"
                } else {
                    "the browser would use this directly (no OCR)"
                }
            );
            println!("{md}");
        }
        Err(e) => {
            eprintln!("convert_text_layer failed: {e}");
            std::process::exit(1);
        }
    }
}
