//! Output regression suite.
//!
//! Every source the suite covers is converted to legacy Markdown, strict
//! Markdown, docling JSON and LaTeX, and compared against the committed
//! fixtures in `tests/data/<format>/expected/`. This pins the Rust converter's
//! output so any unintended change is caught.
//!
//! Sources come from two places, so the upstream corpus is stored once:
//!
//! - `tests/data/<format>/mirror.txt` (in this crate) lists, one file name per
//!   line, the sources taken from the repository-root corpus mirrored from
//!   Python docling, `../../tests/data/<format>/sources/` — the files the
//!   conformance scripts compare against upstream groundtruth. Add a line to
//!   cover another upstream fixture; never copy the file here.
//! - `tests/data/<format>/sources/` (in this crate) holds the suite's **own**
//!   fixtures: formats docling has no backend for, and regression cases of our
//!   own. A file here must not exist in the mirrored corpus.
//!
//! The expected files are keyed by the source's file name, whichever place it
//! came from.
//!
//! The ML formats (PDF, images, METS) need pdfium + the ONNX models, so they are
//! covered by the deterministic snapshot harness (`scripts/conformance/pdf_conformance.sh`)
//! instead of this pure-Rust test.
//!
//! Regenerate the fixtures after an *intentional* output change:
//!
//! ```bash
//! DOCLING_RS_REGEN=1 cargo test -p docling --test regression
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};

/// This crate's `tests/data`: the expected fixtures, the own sources and the
/// `mirror.txt` manifests.
fn data_dir() -> PathBuf {
    // `cargo test` runs with the working directory set to the package root, so
    // resolve there first — this stays correct even if `target/` was copied from
    // another checkout (which leaves env!("CARGO_MANIFEST_DIR"), baked at compile
    // time, pointing at a now-stale absolute path). Fall back to the baked path
    // for non-`cargo test` invocations.
    if let Ok(cwd) = std::env::current_dir() {
        let d = cwd.join("tests/data");
        if d.is_dir() {
            return d;
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

/// The repository-root corpus mirrored from upstream docling (`tests/data` two
/// levels above the crate), which `mirror.txt` entries refer to.
fn mirror_dir() -> PathBuf {
    let d = data_dir().join("../../../../tests/data");
    d.canonicalize().unwrap_or(d)
}

/// A covered source: where to read it, and the format directory whose
/// `expected/` holds its fixtures.
struct Source {
    path: PathBuf,
    fmt_dir: PathBuf,
    /// `<format>/<file>`, for messages.
    rel: String,
}

/// Every covered source, in a stable order: per format, the `mirror.txt`
/// entries (resolved into the mirrored corpus), then the crate's own files.
fn sources() -> Vec<Source> {
    let mut formats: Vec<PathBuf> = fs::read_dir(data_dir())
        .expect("tests/data missing")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    formats.sort();

    let mirror = mirror_dir();
    let mut out = Vec::new();
    for fmt_dir in formats {
        let fmt = fmt_dir.file_name().unwrap().to_string_lossy().to_string();
        let mut names: Vec<(PathBuf, String)> = Vec::new();
        if let Ok(manifest) = fs::read_to_string(fmt_dir.join("mirror.txt")) {
            for name in manifest.lines().map(str::trim).filter(|l| !l.is_empty()) {
                let path = mirror.join(&fmt).join("sources").join(name);
                assert!(
                    path.is_file(),
                    "{fmt}/mirror.txt names {name}, which is not in the mirrored corpus at {}",
                    path.display()
                );
                names.push((path, name.to_string()));
            }
        }
        let own = fmt_dir.join("sources");
        if own.is_dir() {
            let mut files: Vec<PathBuf> = fs::read_dir(&own)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect();
            files.sort();
            for path in files {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                // An own fixture shadowing an upstream one is the duplication
                // this layout exists to prevent (and its expected files would
                // collide): keep it in one place only.
                let twin = mirror.join(&fmt).join("sources").join(&name);
                assert!(
                    !twin.exists(),
                    "{fmt}/sources/{name} also exists in the mirrored corpus — list it in \
                     {fmt}/mirror.txt instead of copying it"
                );
                assert!(
                    !names.iter().any(|(_, n)| *n == name),
                    "{fmt}/sources/{name} is also listed in {fmt}/mirror.txt"
                );
                names.push((path, name));
            }
        }
        for (path, name) in names {
            out.push(Source {
                path,
                rel: format!("{fmt}/{name}"),
                fmt_dir: fmt_dir.clone(),
            });
        }
    }
    out
}

/// `<fmt>/expected/<file><suffix>` for a source.
fn expected_path(src: &Source, suffix: &str) -> PathBuf {
    let name = src.path.file_name().unwrap().to_string_lossy();
    src.fmt_dir.join("expected").join(format!("{name}{suffix}"))
}

fn convert(src: &Path, strict: bool) -> Result<docling::DoclingDocument, String> {
    let source = SourceDocument::from_file(src).map_err(|e| e.to_string())?;
    DocumentConverter::new()
        .strict(strict)
        .convert(source)
        .map(|r| r.document)
        .map_err(|e| e.to_string())
}

/// Convert via the streaming API and concatenate every chunk.
fn stream_to_string(src: &Path, strict: bool) -> Result<String, String> {
    let source = SourceDocument::from_file(src).map_err(|e| e.to_string())?;
    let stream = DocumentConverter::new()
        .strict(strict)
        .convert_streaming(source)
        .map_err(|e| e.to_string())?;
    let mut out = String::new();
    for chunk in stream {
        out.push_str(&chunk.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Streaming Markdown must be byte-identical to the buffered export for every
/// (non-PDF) source — the streaming serializer and the buffered one are held to
/// the same output. (PDF, the format with real page-level streaming, is covered by
/// the snapshot harness and the `StreamAssembler` unit tests in `docling-pdf`.)
#[test]
fn streaming_matches_buffered_markdown() {
    let srcs = sources();
    let mut failures = Vec::new();
    for src in &srcs {
        let rel = &src.rel;
        for strict in [false, true] {
            let buffered = match convert(&src.path, strict) {
                Ok(d) => d.export_to_markdown(),
                Err(e) => {
                    failures.push(format!("{rel}: convert error: {e}"));
                    continue;
                }
            };
            match stream_to_string(&src.path, strict) {
                Ok(streamed) if streamed == buffered => {}
                Ok(_) => failures.push(format!(
                    "{rel} (strict={strict}): streamed Markdown != buffered export"
                )),
                Err(e) => failures.push(format!("{rel} (strict={strict}): stream error: {e}")),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} streaming mismatch(es):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn outputs_match_fixtures() {
    let regen = std::env::var_os("DOCLING_RS_REGEN").is_some();
    let srcs = sources();
    assert!(
        !srcs.is_empty(),
        "no sources under {}",
        data_dir().display()
    );

    let mut failures = Vec::new();
    for src in &srcs {
        let rel = &src.rel;

        let legacy = match convert(&src.path, false) {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{rel}: convert error: {e}"));
                continue;
            }
        };
        let strict = match convert(&src.path, true) {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{rel}: strict convert error: {e}"));
                continue;
            }
        };

        let outputs = [
            (".md", legacy.export_to_markdown()),
            (".strict.md", strict.export_to_markdown()),
            (".json", legacy.export_to_json()),
            (".tex", legacy.export_to_latex()),
        ];
        for (suffix, got) in outputs {
            let path = expected_path(src, suffix);
            if regen {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, got).unwrap();
                continue;
            }
            match fs::read_to_string(&path) {
                Ok(want) if want == got => {}
                Ok(_) => failures.push(format!(
                    "{rel}{suffix}: output changed (run DOCLING_RS_REGEN=1 to update)"
                )),
                Err(_) => {
                    failures.push(format!("{rel}{suffix}: missing fixture {}", path.display()))
                }
            }
        }
    }

    if regen {
        eprintln!("regenerated fixtures for {} sources", srcs.len());
        return;
    }
    assert!(
        failures.is_empty(),
        "{} regression failure(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
