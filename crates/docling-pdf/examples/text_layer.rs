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
            eprintln!(
                "   xref repair: {}",
                docling_pdf::textparse::xref_repair_status(&bytes)
            );
            probe_tail(&bytes);
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
    if cells == 0 {
        eprintln!(
            "   where it is lost:{}",
            docling_pdf::textparse::content_diagnosis(&bytes)
        );
    }
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

/// When the document will not load, show the cross-reference machinery the
/// parser choked on: the last `startxref`, where it points, and what actually
/// sits there. A classic `xref` table, an xref *stream* (`/Type /XRef`), bytes
/// past the end of the file or plain garbage each point at a different repair,
/// and this is enough to tell them apart without handing the PDF around.
fn probe_tail(bytes: &[u8]) {
    let find_last = |needle: &[u8]| {
        bytes
            .windows(needle.len())
            .rposition(|w| w == needle)
            .map(|i| (i, needle.len()))
    };
    let printable = |b: &[u8]| -> String {
        b.iter()
            .map(|&c| {
                if (0x20..0x7f).contains(&c) {
                    c as char
                } else if c == b'\n' || c == b'\r' {
                    '\u{23ce}'
                } else {
                    '.'
                }
            })
            .collect()
    };

    eprintln!("   file: {} bytes", bytes.len());
    eprintln!(
        "   markers: trailer={} startxref={} xref={} /XRef={} %%EOF={}",
        find_last(b"trailer").is_some(),
        find_last(b"startxref").is_some(),
        find_last(b"xref").is_some(),
        find_last(b"/XRef").is_some(),
        find_last(b"%%EOF").is_some(),
    );

    // Trailing bytes after the final %%EOF confuse a strict tail scan.
    if let Some((i, len)) = find_last(b"%%EOF") {
        let after = bytes.len() - (i + len);
        eprintln!("   bytes after the last %%EOF: {after}");
    }

    let Some((sx, _)) = find_last(b"startxref") else {
        eprintln!("   no startxref at all — the xref would have to be rebuilt by scanning objects");
        return;
    };
    let tail = &bytes[sx..bytes.len().min(sx + 60)];
    eprintln!("   last startxref block: {:?}", printable(tail));

    // The number after `startxref` is the byte offset of the xref section.
    let digits: String = tail
        .iter()
        .skip(b"startxref".len())
        .skip_while(|c| c.is_ascii_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .map(|&c| c as char)
        .collect();
    match digits.parse::<usize>() {
        Ok(off) if off < bytes.len() => {
            let end = bytes.len().min(off + 120);
            eprintln!("   at offset {off} -> {:?}", printable(&bytes[off..end]));
        }
        Ok(off) => eprintln!("   startxref points to {off}, past the end of the file"),
        Err(_) => eprintln!("   startxref carries no parseable offset"),
    }
}
