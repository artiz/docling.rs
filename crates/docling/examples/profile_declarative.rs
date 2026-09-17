//! Profiling harness for the declarative (non-ML) formats: for every source in
//! the committed corpus, time the parse (`DocumentConverter::convert`) and the
//! Markdown / JSON exports separately, repeating each until it has run for a
//! while so the numbers are stable. Prints per-format totals and the slowest
//! files. Run: `cargo run --release -p docling --example profile_declarative
//! [-- <format>]`; `PROFILE_MIN_MS` sets the per-measurement floor (300).
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

/// One corpus file's measurements.
struct Row {
    format: String,
    path: PathBuf,
    in_bytes: u64,
    parse_ms: f64,
    md_ms: f64,
    json_ms: f64,
    md_bytes: usize,
    json_bytes: usize,
}

fn time_until(min: Duration, mut f: impl FnMut()) -> Duration {
    let start = Instant::now();
    let mut n = 0u32;
    loop {
        f();
        n += 1;
        let el = start.elapsed();
        if (el >= min && n >= 3) || n >= 200 {
            return el / n;
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
    let mut rows: Vec<Row> = Vec::new();
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
            let hidden = f
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'));
            if SKIP_EXT.contains(&ext.as_str()) || hidden {
                continue;
            }
            let in_bytes = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
            let src = || SourceDocument::from_file(&f).expect("source");
            let Ok(first) = DocumentConverter::new().convert(src()) else {
                eprintln!("skip (convert error): {}", f.display());
                continue;
            };
            let doc = first.document;
            let parse = time_until(min, || {
                let _ = std::hint::black_box(DocumentConverter::new().convert(src()));
            });
            let md = time_until(min, || {
                std::hint::black_box(doc.export_to_markdown());
            });
            let json = time_until(min, || {
                std::hint::black_box(doc.export_to_json());
            });
            rows.push(Row {
                format: fmt.to_string(),
                path: f.clone(),
                in_bytes,
                parse_ms: parse.as_secs_f64() * 1e3,
                md_ms: md.as_secs_f64() * 1e3,
                json_ms: json.as_secs_f64() * 1e3,
                md_bytes: doc.export_to_markdown().len(),
                json_bytes: doc.export_to_json().len(),
            });
        }
    }
    println!(
        "{:<12} {:>5} {:>9} {:>10} {:>9} {:>9} {:>8} {:>8}",
        "format", "files", "in KB", "parse ms", "md ms", "json ms", "md KB", "json KB"
    );
    let mut fmts: Vec<String> = rows.iter().map(|r| r.format.clone()).collect();
    fmts.dedup();
    for fmt in &fmts {
        let rs: Vec<&Row> = rows.iter().filter(|r| &r.format == fmt).collect();
        let sum = |g: fn(&Row) -> f64| rs.iter().map(|r| g(r)).sum::<f64>();
        println!(
            "{:<12} {:>5} {:>9.0} {:>10.2} {:>9.2} {:>9.2} {:>8.0} {:>8.0}",
            fmt,
            rs.len(),
            sum(|r| r.in_bytes as f64) / 1024.0,
            sum(|r| r.parse_ms),
            sum(|r| r.md_ms),
            sum(|r| r.json_ms),
            sum(|r| r.md_bytes as f64) / 1024.0,
            sum(|r| r.json_bytes as f64) / 1024.0
        );
    }
    let rel = |r: &Row| {
        r.path
            .strip_prefix(&root)
            .unwrap_or(&r.path)
            .display()
            .to_string()
    };
    let per_kb = |ms: f64, bytes: usize| ms / (bytes.max(1) as f64 / 1024.0);
    println!("\nslowest parse:");
    rows.sort_by(|a, b| b.parse_ms.total_cmp(&a.parse_ms));
    for r in rows.iter().take(12) {
        println!(
            "  {:>9.2} ms  {:>7} KB  {}",
            r.parse_ms,
            r.in_bytes / 1024,
            rel(r)
        );
    }
    println!("\nslowest markdown export (ms, per KB of output):");
    rows.sort_by(|a, b| per_kb(b.md_ms, b.md_bytes).total_cmp(&per_kb(a.md_ms, a.md_bytes)));
    for r in rows.iter().take(8) {
        println!(
            "  {:>9.3} ms  {:>7} KB out  {:.4} ms/KB  {}",
            r.md_ms,
            r.md_bytes / 1024,
            per_kb(r.md_ms, r.md_bytes),
            rel(r)
        );
    }
    println!("\nslowest json export (ms, per KB of output):");
    rows.sort_by(|a, b| {
        per_kb(b.json_ms, b.json_bytes).total_cmp(&per_kb(a.json_ms, a.json_bytes))
    });
    for r in rows.iter().take(8) {
        println!(
            "  {:>9.3} ms  {:>7} KB out  {:.4} ms/KB  {}",
            r.json_ms,
            r.json_bytes / 1024,
            per_kb(r.json_ms, r.json_bytes),
            rel(r)
        );
    }
}
