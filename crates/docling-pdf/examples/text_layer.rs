//! Run **exactly** what a wasm build does with a PDF: `convert_text_layer` —
//! the pure-Rust content-stream parser, no pdfium, no ONNX. Use it to tell a
//! browser "this PDF needs OCR" apart from a genuine scan: the browser falls
//! back to OCR precisely when this produces no nodes, so an empty result here
//! reproduces that decision offline.
//!
//! Note this is *not* what the CLI's `--no-ocr` runs — that goes through
//! pdfium's text extraction, which can read layers this parser cannot. When the
//! two disagree, the breakdown below says which stage lost the text.
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

    // Stage 1 — does lopdf load the document at all? `textparse` swallows this
    // failure (an unloadable PDF and a scan both come out empty), so ask
    // directly: encryption, a damaged xref or an unsupported filter all land
    // here, and they mean something very different from "no text layer".
    match lopdf::Document::load_mem(&bytes) {
        Err(e) => {
            eprintln!("1. lopdf load: FAILED — {e}");
            eprintln!("   (the browser reports this as \"no embedded text layer\")");
        }
        Ok(doc) => {
            eprintln!(
                "1. lopdf load: ok — {} page(s), version {}, encrypted: {}",
                doc.get_pages().len(),
                doc.version,
                doc.is_encrypted()
            );
        }
    }

    // Stage 2 — raw line cells out of the content streams. Zero here with a
    // successful load means the text is there but we cannot decode it (font
    // encoding, an unhandled operator, a filter), not that the page is a scan.
    let pages = docling_pdf::textparse::pdf_textlines(&bytes);
    let cells: usize = pages.iter().map(|(_, _, c)| c.len()).sum();
    eprintln!("2. text lines: {cells} across {} page(s)", pages.len());
    for (i, (_, _, c)) in pages.iter().enumerate().take(3) {
        if let Some(first) = c.iter().find(|c| !c.text.trim().is_empty()) {
            eprintln!("   page {}: first line {:?}", i + 1, first.text);
        }
    }

    // Stage 3 — the assembled document, i.e. what the browser actually gets.
    match docling_pdf::convert_text_layer(&bytes, &name) {
        Ok(doc) => {
            let md = doc.export_to_markdown();
            eprintln!(
                "3. convert_text_layer: {} node(s), {} chars — {}",
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
            eprintln!("3. convert_text_layer: FAILED — {e}");
            std::process::exit(1);
        }
    }
}
