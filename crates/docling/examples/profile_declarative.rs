//! Profiling harness for the declarative (non-ML) formats: for every source in
//! the committed corpus, time the parse (`DocumentConverter::convert`) and the
//! Markdown / JSON exports separately, repeating each until it has run for a
//! while so the numbers are stable. Prints per-format totals and the slowest
//! files. Run: `cargo run --release -p docling --example profile_declarative`.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use docling::{DocumentConverter, SourceDocument};

const FORMATS: &[&str] = &[
    "docx",
    "xlsx",
    "pptx",
    "html",
    "md",
    "csv",
    "epub",
    "asciidoc",
    "rtf",
    "webvtt",
    "odf",
    "uspto",
    "jats",
    "xbrl",
    "email",
    "mhtml",
    "doclang",
    "xls",
    "doc",
    "ppt",
    "iwork",
    "latex",
    "md_deepseek",
    "json_dots",
    "html_chandra",
    "ebcdic",
];
const SKIP_EXT: &[&str] = &[
    "pdf", "png", "jpg", "jpeg", "gif", "webp", "tif", "tiff", "heic", "svg", "json", "txt", "sty",
    "bib", "bst", "cls", "log", "aux", "toc", "out", "bbl", "blg",
];

fn time_until(min: Duration, mut f: impl FnMut()) -> (Duration, u32) {
    let start = Instant::now();
    let mut n = 0u32;
    loop {
        f();
        n += 1;
        let el = start.elapsed();
        if el >= min && n >= 3 || n >= 200 {
            return (el / n, n);
        }
    }
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data");
    let min = Duration::from_millis(
        std::env::var("PROFILE_MIN_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300),
    );
    let only: Option<String> = std::env::args().nth(1);
    let mut rows: Vec<(String, PathBuf, u64, f64, f64, f64, usize, usize)> = Vec::new();
    for fmt in FORMATS {
        if only.as_deref().is_some_and(|o| o != *fmt) {
            continue;
        }
        let dir = root.join(fmt).join("sources");
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        for f in files {
            let ext = f
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if SKIP_EXT.contains(&ext.as_str())
                || f.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'))
            {
                continue;
            }
            let bytes = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
            let src = || SourceDocument::from_file(&f).expect("source");
            let Ok(first) = DocumentConverter::new().convert(src()) else {
                eprintln!("skip (convert error): {}", f.display());
                continue;
            };
            let doc = first.document;
            let (parse, _) = time_until(min, || {
                let _ = std::hint::black_box(DocumentConverter::new().convert(src()));
            });
            let (md, _) = time_until(min, || {
                std::hint::black_box(doc.export_to_markdown());
            });
            let (json, _) = time_until(min, || {
                std::hint::black_box(doc.export_to_json());
            });
            let md_len = doc.export_to_markdown().len();
            let json_len = doc.export_to_json().len();
            rows.push((
                fmt.to_string(),
                f.clone(),
                bytes,
                parse.as_secs_f64() * 1e3,
                md.as_secs_f64() * 1e3,
                json.as_secs_f64() * 1e3,
                md_len,
                json_len,
            ));
        }
    }
    println!(
        "{:<12} {:>5} {:>9} {:>10} {:>9} {:>9} {:>8} {:>8}",
        "format", "files", "in KB", "parse ms", "md ms", "json ms", "md KB", "json KB"
    );
    let mut fmts: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
    fmts.dedup();
    for fmt in &fmts {
        let rs: Vec<_> = rows.iter().filter(|r| &r.0 == fmt).collect();
        let s = |g: &dyn Fn(&&(String, PathBuf, u64, f64, f64, f64, usize, usize)) -> f64| {
            rs.iter().map(g).sum::<f64>()
        };
        println!(
            "{:<12} {:>5} {:>9.0} {:>10.2} {:>9.2} {:>9.2} {:>8.0} {:>8.0}",
            fmt,
            rs.len(),
            s(&|r| r.2 as f64) / 1024.0,
            s(&|r| r.3),
            s(&|r| r.4),
            s(&|r| r.5),
            s(&|r| r.6 as f64) / 1024.0,
            s(&|r| r.7 as f64) / 1024.0
        );
    }
    println!("\nslowest parse:");
    rows.sort_by(|a, b| b.3.total_cmp(&a.3));
    for r in rows.iter().take(12) {
        println!(
            "  {:>9.2} ms  {:>7} KB  {}",
            r.3,
            r.2 / 1024,
            r.1.strip_prefix(&root).unwrap_or(&r.1).display()
        );
    }
    println!("\nslowest markdown export (ms, per KB of output):");
    rows.sort_by(|a, b| (b.4 / b.6.max(1) as f64).total_cmp(&(a.4 / a.6.max(1) as f64)));
    for r in rows.iter().take(8) {
        println!(
            "  {:>9.3} ms  {:>7} KB out  {:.4} ms/KB  {}",
            r.4,
            r.6 / 1024,
            r.4 / (r.6.max(1) as f64 / 1024.0),
            r.1.strip_prefix(&root).unwrap_or(&r.1).display()
        );
    }
    println!("\nslowest json export (ms, per KB of output):");
    rows.sort_by(|a, b| (b.5 / b.7.max(1) as f64).total_cmp(&(a.5 / a.7.max(1) as f64)));
    for r in rows.iter().take(8) {
        println!(
            "  {:>9.3} ms  {:>7} KB out  {:.4} ms/KB  {}",
            r.5,
            r.7 / 1024,
            r.5 / (r.7.max(1) as f64 / 1024.0),
            r.1.strip_prefix(&root).unwrap_or(&r.1).display()
        );
    }
}
