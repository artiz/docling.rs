//! Dump layout regions as (label, l, t, r, b) in page points + page size, for
//! reverse-engineering docling's DocLang 0-500 location normalization.
use docling_pdf::layout::LayoutModel;
use docling_pdf::PdfDocument;

fn main() {
    let path = std::env::args().nth(1).expect("pdf");
    let bytes = std::fs::read(&path).expect("read");
    let doc = PdfDocument::open(&bytes, None).expect("open");
    let mut layout = LayoutModel::load().expect("layout");
    for (pi, page) in doc.pages.iter().enumerate() {
        println!("# page {} size = {:.1} x {:.1} pt", pi + 1, page.width, page.height);
        let regions = layout
            .predict(&page.image, page.width, page.height)
            .expect("layout");
        for r in &regions {
            let txt: String = page
                .cells
                .iter()
                .filter(|c| {
                    let (cx, cy) = ((c.l + c.r) / 2.0, (c.t + c.b) / 2.0);
                    cx >= r.l && cx <= r.r && cy >= r.t && cy <= r.b
                })
                .map(|c| c.text.trim())
                .collect::<Vec<_>>()
                .join(" ");
            let head: String = txt.chars().take(38).collect();
            println!(
                "{:>14} l={:6.1} t={:6.1} r={:6.1} b={:6.1} | {}",
                r.label, r.l, r.t, r.r, r.b, head
            );
        }
    }
}
