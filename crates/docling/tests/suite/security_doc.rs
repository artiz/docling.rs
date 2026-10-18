//! `docs/SECURITY.md` cannot drift from the code (#619).
//!
//! `docs/env-vars.csv` is the registry of every `DOCLING_*` environment
//! variable the crates read: its name, a category and the source file that
//! reads it. The test holds four things together:
//!
//! 1. every `"DOCLING_…"` literal in a crate's `src/` is in the registry — a
//!    new variable has to be categorized when it is added;
//! 2. every registry row is still read where it says (no stale rows);
//! 3. every `limit`, `subprocess`, `secret` and `network` variable is named
//!    in `docs/SECURITY.md` — the categories an operator hardening a
//!    deployment must know about;
//! 4. every `DOCLING_…` name `SECURITY.md` mentions still exists in the code.
//!
//! The categories are a judgement made once per variable in the CSV:
//! `model path`, `operational`, `performance` and `debug` variables are
//! documented in README / docs/DEPLOYMENT.md rather than here.
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The registry categories `SECURITY.md` must cover.
const SECURITY_CATEGORIES: &[&str] = &["limit", "subprocess", "secret", "network"];

/// All categories a registry row may carry.
const CATEGORIES: &[&str] = &[
    "limit",
    "subprocess",
    "secret",
    "network",
    "model path",
    "operational",
    "performance",
    "debug",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every `.rs` file under each crate's `src/` (tests, examples and benches
/// are left out: the variables they set are not ones the product reads).
fn source_files() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    for krate in fs::read_dir(root().join("crates")).unwrap().flatten() {
        walk(&krate.path().join("src"), &mut out);
    }
    out.sort();
    out
}

/// The `DOCLING_…` names quoted as string literals in `text`.
fn quoted_names(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (i, _) in text.match_indices("\"DOCLING_") {
        let rest = &text[i + 1..];
        let end = rest
            .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(rest.len());
        if rest[end..].starts_with('"') {
            names.insert(rest[..end].to_string());
        }
    }
    names
}

/// The `DOCLING_…` names mentioned anywhere in `text` (prose, tables, code).
fn mentioned_names(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (i, _) in text.match_indices("DOCLING_") {
        let rest = &text[i..];
        let end = rest
            .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(rest.len());
        let name = rest[..end].trim_end_matches('_');
        // `DOCLING_*` / `DOCLING_RS_ZIP_MAX_*` are families, not names.
        if name.len() > "DOCLING_".len() && !rest[end..].starts_with('*') {
            names.insert(name.to_string());
        }
    }
    names
}

struct Row {
    name: String,
    category: String,
    source: String,
}

fn registry() -> Vec<Row> {
    let csv = fs::read_to_string(root().join("docs/env-vars.csv")).expect("docs/env-vars.csv");
    let mut lines = csv.lines();
    assert_eq!(lines.next(), Some("name,category,source"), "CSV header");
    lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let cols: Vec<&str> = l.split(',').collect();
            assert_eq!(cols.len(), 3, "registry row {l:?}: name,category,source");
            assert!(
                CATEGORIES.contains(&cols[1]),
                "registry row {l:?}: unknown category (one of {CATEGORIES:?})"
            );
            Row {
                name: cols[0].to_string(),
                category: cols[1].to_string(),
                source: cols[2].to_string(),
            }
        })
        .collect()
}

#[test]
fn every_variable_the_code_reads_is_registered() {
    let rows = registry();
    let registered: BTreeSet<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    let mut missing = Vec::new();
    for file in source_files() {
        let text = fs::read_to_string(&file).unwrap();
        for name in quoted_names(&text) {
            // `docling-core::env`'s own unit tests exercise the parser with
            // throwaway names.
            if name.starts_with("DOCLING_TEST_") || registered.contains(name.as_str()) {
                continue;
            }
            missing.push(format!("{name} ({})", file.display()));
        }
    }
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "add these to docs/env-vars.csv with a category (and to docs/SECURITY.md \
         if it is a limit, subprocess, secret or network variable):\n{}",
        missing.join("\n")
    );
}

#[test]
fn every_registered_variable_is_read_where_the_registry_says() {
    let mut stale = Vec::new();
    for row in registry() {
        let text = fs::read_to_string(root().join(&row.source)).unwrap_or_default();
        if !text.contains(&format!("\"{}\"", row.name)) {
            stale.push(format!("{} ({})", row.name, row.source));
        }
    }
    assert!(
        stale.is_empty(),
        "docs/env-vars.csv rows whose source no longer reads them — fix the \
         path, or drop the row (and its SECURITY.md mention):\n{}",
        stale.join("\n")
    );
}

#[test]
fn security_md_names_every_hardening_variable_and_only_real_ones() {
    let doc = fs::read_to_string(root().join("docs/SECURITY.md")).unwrap();
    let mentioned = mentioned_names(&doc);
    let rows = registry();
    let undocumented: Vec<&str> = rows
        .iter()
        .filter(|r| SECURITY_CATEGORIES.contains(&r.category.as_str()))
        .filter(|r| !mentioned.contains(&r.name))
        .map(|r| r.name.as_str())
        .collect();
    assert!(
        undocumented.is_empty(),
        "docs/SECURITY.md does not name these {SECURITY_CATEGORIES:?} variables: {undocumented:?}"
    );
    let known: BTreeSet<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    let unknown: Vec<&String> = mentioned
        .iter()
        .filter(|n| !known.contains(n.as_str()))
        .collect();
    assert!(
        unknown.is_empty(),
        "docs/SECURITY.md names variables the code does not read: {unknown:?}"
    );
}
