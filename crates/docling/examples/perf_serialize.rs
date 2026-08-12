//! Serializer profiling harness (#216 follow-up): convert once, serialize in
//! a loop so Markdown / DocLang / JSON generation dominates the profile.
//!
//! ```bash
//! cargo build --release --no-default-features --example perf_serialize
//! target/release/examples/perf_serialize <file> [iters]
//! valgrind --tool=callgrind target/release/examples/perf_serialize <file> 20
//! ```

use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: perf_serialize <file> [iters]");
    let iters: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(100);

    let source = docling::SourceDocument::from_file(&path).expect("read");
    let t0 = Instant::now();
    let result = docling::DocumentConverter::new()
        .convert(source)
        .expect("convert");
    let document = result.document;
    println!("convert: {:?}", t0.elapsed());

    let mut sink = 0usize;
    for (name, f) in [
        (
            "markdown",
            Box::new(|d: &docling::DoclingDocument| d.export_to_markdown().len())
                as Box<dyn Fn(&docling::DoclingDocument) -> usize>,
        ),
        ("doclang", Box::new(|d| d.export_to_doclang().len())),
        ("json", Box::new(|d| d.export_to_json().len())),
    ] {
        let t = Instant::now();
        for _ in 0..iters {
            sink += f(&document);
        }
        let dt = t.elapsed();
        println!("{name}: {:?}/iter ({iters} iters)", dt / iters as u32);
    }
    eprintln!("(sink {sink})");
}
