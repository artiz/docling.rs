//! Pandoc AST output (#515) pinned two ways for a set of documents chosen
//! to cover the mapping — headings, nested and ordered lists, inline
//! formatting and links, code blocks, inline and display formulas,
//! checkboxes, tables with spans / header rows / rich cells, key-value forms
//! and definition lists:
//!
//! - `tests/data/pandoc/<name>.pandoc.json` — the exact `--to pandoc` output,
//!   always compared;
//! - `tests/data/pandoc/<name>.native` — what `pandoc -f json -t native`
//!   reads from it, the acceptance check that Pandoc itself takes the
//!   document as meant. Compared when a `pandoc` binary is on `PATH` (or
//!   named by `PANDOC`); skipped otherwise, so CI without Pandoc stays green.
//!
//! Regenerate after an intentional change (the natives need Pandoc 3.x):
//!
//! ```bash
//! DOCLING_RS_REGEN=1 cargo test -p docling --test suite pandoc::
//! ```
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use docling::{DocumentConverter, SourceDocument};

/// `<format>/<file>` under the repository-root corpus.
const CASES: &[&str] = &[
    "md/nested.md",
    "md/inline_and_formatting.md",
    "md/blocks.md",
    "docx/unit_test_formatting.docx",
    "docx/equations.docx",
    "docx/docx_code_blocks.docx",
    "docx/word_tables.docx",
    "docx/docx_checkboxes.docx",
    "html/html_rich_table_cells.html",
    "html/table_03.html",
    "html/kvp_data_example.html",
    "html/html_description_list.html",
    "latex/example_01.tex",
    // Our own fixtures (crates/docling/tests/data): pictures that must stay
    // `Image`s (#537), footnotes / endnotes as `Note`s at their calls (#538).
    "docx/pandoc_images.docx",
    "docx/pandoc_footnotes.docx",
    "odf/footnotes.odt",
    // #833: a formatted, linked caption and a multi-run list item with a
    // nested list; a hyperlink in a one-paragraph table cell; a multi-run
    // caption and heading from a docling JSON document.
    "html/pandoc_inlines.html",
    "docx/pandoc_table_cell_hyperlink.docx",
    "json_docling/pandoc_caption_heading_inline.json",
    // #834: a picture's alt text and displayed size, DOCX and ODT.
    "docx/pandoc_picture_alt_dim.docx",
    "odf/pandoc_picture_alt_dim.odt",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A case's source: the repository-root corpus (upstream's fixtures), else
/// this crate's own `tests/data/<format>/sources/`.
fn source(fmt: &str, file: &str) -> PathBuf {
    let upstream = root()
        .join("tests/data")
        .join(fmt)
        .join("sources")
        .join(file);
    if upstream.exists() {
        return upstream;
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(fmt)
        .join("sources")
        .join(file)
}

fn expected_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/pandoc")
}

fn pandoc_bin() -> Option<String> {
    let bin = std::env::var("PANDOC").unwrap_or_else(|_| "pandoc".to_string());
    let ok = Command::new(&bin)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    ok.then_some(bin)
}

/// Pandoc's `native` reading of `json`, or its error.
fn native(bin: &str, json: &str) -> Result<String, String> {
    let mut child = Command::new(bin)
        .args(["-f", "json", "-t", "native"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(json.as_bytes())
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

#[test]
fn pandoc_output_matches_the_stored_references() {
    let regen = std::env::var_os("DOCLING_RS_REGEN").is_some();
    let pandoc = pandoc_bin();
    if pandoc.is_none() {
        eprintln!("pandoc not found: comparing the JSON only, not Pandoc's reading of it");
    }
    let converter = DocumentConverter::new();
    let mut failures = Vec::new();
    for case in CASES {
        let (fmt, file) = case.split_once('/').expect("<format>/<file>");
        let src = source(fmt, file);
        let doc = converter
            .convert(SourceDocument::from_file(&src).expect("read source"))
            .unwrap_or_else(|e| panic!("{case}: {e}"))
            .document;
        let json = doc.export_to_pandoc_json();

        let json_path = expected_dir().join(format!("{file}.pandoc.json"));
        let native_path = expected_dir().join(format!("{file}.native"));
        if regen {
            fs::create_dir_all(expected_dir()).expect("mkdir");
            fs::write(&json_path, &json).expect("write json");
            if let Some(bin) = &pandoc {
                let n = native(bin, &json).unwrap_or_else(|e| panic!("{case}: pandoc: {e}"));
                fs::write(&native_path, n).expect("write native");
            }
            continue;
        }
        match fs::read_to_string(&json_path) {
            Ok(want) if want == json => {}
            Ok(_) => failures.push(format!(
                "{case}: Pandoc JSON changed (DOCLING_RS_REGEN=1 to update)"
            )),
            Err(_) => failures.push(format!("{case}: missing {}", json_path.display())),
        }
        if let Some(bin) = &pandoc {
            match native(bin, &json) {
                Ok(got) => {
                    let want = fs::read_to_string(&native_path).unwrap_or_default();
                    if got != want {
                        failures.push(format!("{case}: pandoc's native reading changed"));
                    }
                }
                Err(e) => failures.push(format!("{case}: pandoc rejected the output: {e}")),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Count the AST nodes of constructor `t`.
fn count(v: &serde_json::Value, t: &str) -> usize {
    match v {
        serde_json::Value::Object(o) => {
            usize::from(o.get("t").and_then(|x| x.as_str()) == Some(t))
                + o.values().map(|x| count(x, t)).sum::<usize>()
        }
        serde_json::Value::Array(a) => a.iter().map(|x| count(x, t)).sum(),
        _ => 0,
    }
}

fn convert(fmt: &str, file: &str) -> docling::DoclingDocument {
    DocumentConverter::new()
        .convert(SourceDocument::from_file(source(fmt, file)).expect("read source"))
        .expect("convert")
        .document
}

/// The issues' acceptance checks, end to end: #537's PNG / GIF / EMF are
/// three `Image`s (the EMF, which has no decodable payload, a placeholder),
/// and a DOCX that `pandoc` rebuilds from the AST holds the two raster
/// pictures; #538's five notes are `Note`s and land in the rebuilt DOCX's
/// `word/footnotes.xml` / `endnotes.xml`. The rebuild needs a `pandoc`
/// binary and is skipped without one.
#[test]
fn a_docx_rebuilt_from_the_ast_keeps_pictures_and_notes() {
    let images = convert("docx", "pandoc_images.docx").export_to_pandoc_json();
    let notes = convert("docx", "pandoc_footnotes.docx").export_to_pandoc_json();
    let ast: serde_json::Value = serde_json::from_str(&images).unwrap();
    assert_eq!(count(&ast, "Image"), 3, "{images}");
    assert_eq!(count(&ast, "RawBlock"), 0, "{images}");
    assert_eq!(
        images.matches("data:image/").count(),
        2,
        "PNG + GIF embedded"
    );
    let ast: serde_json::Value = serde_json::from_str(&notes).unwrap();
    assert_eq!(count(&ast, "Note"), 5, "{notes}");

    let Some(bin) = pandoc_bin() else {
        eprintln!("pandoc not found: skipping the DOCX rebuild");
        return;
    };
    let dir = std::env::temp_dir().join(format!("docling-pandoc-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let rebuild = |name: &str, json: &str| -> Vec<String> {
        let out = dir.join(format!("{name}.docx"));
        let mut child = Command::new(&bin)
            .args(["-f", "json", "-t", "docx", "-o"])
            .arg(&out)
            .stdin(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("run pandoc");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(json.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success(), "pandoc -t docx failed");
        let mut zip = zip::ZipArchive::new(fs::File::open(&out).unwrap()).unwrap();
        let names: Vec<String> = zip.file_names().flatten().map(|n| n.into_owned()).collect();
        let mut parts = names.clone();
        for part in ["word/footnotes.xml", "word/document.xml"] {
            if let Ok(mut f) = zip.by_name(part) {
                let mut xml = String::new();
                std::io::Read::read_to_string(&mut f, &mut xml).unwrap();
                parts.push(xml);
            }
        }
        parts
    };
    let parts = rebuild("images", &images);
    let media = parts
        .iter()
        .filter(|n| n.starts_with("word/media/"))
        .count();
    assert!(media >= 2, "{media} media parts in the rebuilt DOCX");
    let parts = rebuild("notes", &notes);
    let footnotes = parts
        .iter()
        .find(|p| p.contains("<w:footnotes"))
        .expect("footnotes.xml");
    for mark in [
        "MARKFN",
        "MARKEN",
        "Note on the heading",
        "Note on the list item",
    ] {
        assert!(
            footnotes.contains(mark),
            "{mark} missing from footnotes.xml"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}
