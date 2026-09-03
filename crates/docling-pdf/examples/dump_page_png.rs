//! Dump the pipeline's rendered page bitmap to a PNG — debugging aid for
//! orientation/raster issues (what does layout/OCR actually see?).
//!
//! Usage: cargo run -p docling-pdf --features ml --example dump_page_png -- file.pdf out.png

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: dump_page_png <pdf> <out.png>");
    let out = std::env::args().nth(2).expect("out.png required");
    let bytes = std::fs::read(&path).expect("read pdf");
    docling_pdf::pdfium_backend::for_each_page(
        &bytes,
        None,
        true,
        true,
        None,
        |i, _total, page| {
            if i == 0 {
                let img = &page.image;
                println!(
                    "page 0: {}x{} pt, image {}x{} px",
                    page.width,
                    page.height,
                    img.width(),
                    img.height()
                );
                img.save(&out).expect("save png");
            }
            Ok(())
        },
    )
    .map_err(|e: docling_pdf::PdfError| e)
    .expect("convert");
}
