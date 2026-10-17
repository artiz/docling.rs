//! Output regression suite.
//!
//! Every source the suite covers is converted to legacy Markdown, strict
//! Markdown, docling JSON, LaTeX and HTML, and compared against the committed
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
//! The ML formats (PDF, images, METS) need the ONNX models, so they are
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

/// `<fmt>/expected/<file><suffix>` for a source. A mirrored source inside a
/// directory (`1706.03762/main.tex`) takes upstream's fixture name for it,
/// the path with `_` for `/` (`1706.03762_main.tex`).
fn expected_path(src: &Source, suffix: &str) -> PathBuf {
    let name = src
        .rel
        .split_once('/')
        .map_or(src.rel.as_str(), |(_, name)| name)
        .replace('/', "_");
    src.fmt_dir.join("expected").join(format!("{name}{suffix}"))
}

/// A converter for `src`. An XBRL instance is paired with the `*-taxonomy`
/// directory beside it that holds the schema its `link:schemaRef` names — the
/// layout upstream's `tests/data/xbrl/sources/` keeps its fixtures in, whose
/// test passes that directory as the backend's `taxonomy` option.
fn converter_for(src: &Path) -> DocumentConverter {
    let mut converter = DocumentConverter::new();
    if let Some(taxonomy) = xbrl_taxonomy_dir(src) {
        converter = converter.xbrl_taxonomy(taxonomy);
    }
    converter
}

fn xbrl_taxonomy_dir(src: &Path) -> Option<PathBuf> {
    let dir = src.parent()?;
    if dir.parent()?.file_name()? != "xbrl" {
        return None;
    }
    let text = fs::read_to_string(src).ok()?;
    let href = text
        .split("schemaRef")
        .nth(1)?
        .split("href=\"")
        .nth(1)?
        .split('"')
        .next()?;
    fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.to_string_lossy().ends_with("-taxonomy"))
        .find(|p| p.join(href).is_file())
}

fn convert(src: &Path, strict: bool) -> Result<docling::DoclingDocument, String> {
    let source = SourceDocument::from_file(src).map_err(|e| e.to_string())?;
    converter_for(src)
        .strict(strict)
        .convert(source)
        .map(|r| r.document)
        .map_err(|e| e.to_string())
}

/// Convert via the streaming API and concatenate every chunk.
fn stream_to_string(src: &Path, strict: bool) -> Result<String, String> {
    let source = SourceDocument::from_file(src).map_err(|e| e.to_string())?;
    let stream = converter_for(src)
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

/// `convert_streaming_with_options` with every layer and picture traversal
/// on — the CLI's streamed `--layers all --traverse-pictures` — matches the
/// buffered `export_to_markdown_with_options` byte for byte.
#[test]
fn streaming_with_options_matches_buffered_markdown() {
    let options = docling::MarkdownExportOptions {
        layers: docling::ContentLayers::ALL,
        traverse_pictures: true,
        ..Default::default()
    };
    let mut failures = Vec::new();
    for src in &sources() {
        let rel = &src.rel;
        let buffered = match convert(&src.path, false) {
            Ok(d) => d.export_to_markdown_with_options(&options).0,
            Err(e) => {
                failures.push(format!("{rel}: convert error: {e}"));
                continue;
            }
        };
        let streamed = SourceDocument::from_file(&src.path)
            .map_err(|e| e.to_string())
            .and_then(|source| {
                converter_for(&src.path)
                    .convert_streaming_with_options(source, &options)
                    .map_err(|e| e.to_string())
            })
            .and_then(|stream| {
                stream
                    .collect::<Result<String, _>>()
                    .map_err(|e| e.to_string())
            });
        match streamed {
            Ok(streamed) if streamed == buffered => {}
            Ok(_) => failures.push(format!("{rel}: streamed Markdown != buffered export")),
            Err(e) => failures.push(format!("{rel}: stream error: {e}")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} streaming mismatch(es):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Sources whose pictures are metafiles (EMF/WMF) with text in them,
/// rasterized with the host's fonts (`backend/metafile.rs` loads the system
/// fonts): the PNG bytes differ where the font set does — a WSL host that
/// sees `C:\Windows\Fonts` draws the text in Arial, the fixture's host
/// drew it in a substitute. For these the embedded PNG payloads are left out
/// of the JSON / HTML comparison; everything else, the pictures' sizes
/// included, still has to match.
const HOST_FONT_PICTURES: &[&str] = &["doc/embedded_word_object.doc"];

/// `text` with every `data:image/png;base64,` payload replaced by `…`.
fn mask_png_payloads(text: &str) -> String {
    const PREFIX: &str = "data:image/png;base64,";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(PREFIX) {
        out.push_str(&rest[..at + PREFIX.len()]);
        out.push('…');
        rest = rest[at + PREFIX.len()..].trim_start_matches(|c: char| {
            c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')
        });
    }
    out.push_str(rest);
    out
}

/// The masking drops the PNG payloads and nothing else.
#[test]
fn png_payload_masking_keeps_the_rest() {
    let a = r#"{"uri": "data:image/png;base64,iVBORw0+/=", "size": {"width": 2048.0}}"#;
    let b = r#"{"uri": "data:image/png;base64,QUJD", "size": {"width": 2048.0}}"#;
    let c = r#"{"uri": "data:image/png;base64,QUJD", "size": {"width": 1024.0}}"#;
    assert_eq!(mask_png_payloads(a), mask_png_payloads(b));
    assert_ne!(mask_png_payloads(b), mask_png_payloads(c));
    assert_eq!(
        mask_png_payloads(r#"<img src="data:image/png;base64,QUJD" width="9">"#),
        r#"<img src="data:image/png;base64,…" width="9">"#
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
            (
                ".html",
                legacy
                    .export_to_html_with_images(docling::ImageMode::Embedded, "artifacts")
                    .0,
            ),
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
                Ok(want)
                    if HOST_FONT_PICTURES.contains(&rel.as_str())
                        && mask_png_payloads(&want) == mask_png_payloads(&got) => {}
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
