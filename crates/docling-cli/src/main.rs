//! Minimal CLI: convert a file and print Markdown or JSON to stdout.
//!
//! The docling.rs counterpart of `docling.cli.main`; `docling-rs serve`
//! (with `--features serve`) starts the HTTP conversion API.
//!
//! Usage: docling-rs [--strict] [--to md|json] [--pages A-B] [--images MODE] [--input GLOB --output DIR [--jobs N]] [--fetch-images] [--no-stream] [--no-table-former] [--no-ocr] [--force-full-page-ocr] [--no-text-panels] [--ocr-lang en|ch] [--pipeline standard|vlm] [--vlm-endpoint URL] [--vlm-model NAME] [--asr-model PRESET] [--asr-lang CODE] [--video-frames N] [--use-web-browser] [--enrich-picture-classes] [--enrich-code] [--enrich-formula] <input-file>
//!   --input GLOB       batch mode (#205): convert every file the glob matches
//!                      (`--input '/data/reports/**/*.pdf'` — quote it so the
//!                      shell doesn't expand it) instead of one positional file.
//!                      One warm process converts them all: the PDF/image ML
//!                      pipeline loads its models once and is reused for every
//!                      file, like docling-serve's warm pipeline.
//!   --output DIR       where batch results land. The directory structure below
//!                      the pattern's static prefix is preserved (`a/b/x.pdf`
//!                      under `--input '/data/**/*.pdf'` becomes
//!                      `DIR/a/b/x.md`); extensions follow `--to` (`.md`,
//!                      `.json`, `.dclx`, `.chunks.json`). Also works with a
//!                      single positional input file. Output paths print to
//!                      stdout one per line; progress goes to stderr. A failed
//!                      file is reported and skipped (exit code 1 at the end).
//!   --jobs N           batch workers (default 1). Declarative formats convert
//!                      in parallel; PDF/image files share the one warm ML
//!                      pipeline (which parallelizes internally per document).
//!   --to md|json       output format (default: md). `json` emits docling-core's
//!                      native DoclingDocument JSON (export_to_dict).
//!   --pages A-B        convert only PDF pages A through B (1-based, inclusive;
//!                      a single page number also works). Skipped pages are
//!                      never rasterized, so a small window over a huge PDF is
//!                      cheap. Non-PDF inputs ignore this.
//!   --images MODE      picture handling for Markdown (mirrors docling's
//!                      image_mode): placeholder (default) | embedded | referenced.
//!                      `referenced` writes image files under ./artifacts/ —
//!                      streamed to disk page by page, so image-heavy PDFs stay
//!                      memory-bounded. JSON always embeds extracted images as
//!                      data URIs.
//!   --fetch-images     for HTML/EPUB, resolve external <img src> (data: URIs,
//!                      local files, http(s) URLs, EPUB archive entries) and embed
//!                      the bytes. Off by default; fetches over the network.
//!   --strict           cleaner, more conformant Markdown instead of byte-for-byte
//!                      docling-legacy output (Markdown only).
//!   --no-stream        build the whole document before printing Markdown instead
//!                      of streaming it page by page. Streaming is the default for
//!                      Markdown (placeholder/embedded images); JSON and referenced
//!                      images always use the buffered path.
//!   --no-table-former  skip loading/running the TableFormer table-structure
//!                      model for PDF/image input; tables fall back to simple
//!                      geometric reconstruction from cell positions. Faster
//!                      (no model load, no per-table inference) at the cost of
//!                      table fidelity — helps most in streaming mode.
//!   --video-frames N   Max frames sampled from a video input as timestamped
//!                      pictures (needs the ffmpeg binary; 0 = transcript
//!                      only). Default 8.
//!   --asr-model NAME   Whisper preset for audio inputs: whisper_tiny_en,
//!                      whisper_base_en, whisper_small_en, whisper_distil_small_en
//!                      (models under models/asr/<preset>/; fetch them with
//!                      download_dependencies.sh --asr-model=<preset>)
//!   --asr-lang CODE    transcription language for audio/video input: a Whisper
//!                      code (en, de, zh, ...) or auto (the default) to detect
//!                      it from the first 30 seconds. English-only presets
//!                      always transcribe English.
//!   --force-full-page-ocr  OCR every PDF page even when it has a text layer
//!                      (docling's force_full_page_ocr) — for layers that lie:
//!                      broken encodings, forms with a few typed-in fields
//!   --no-text-panels   keep every detected picture as a picture — disable the
//!                      #157 demotion of uncaptioned text-panel pictures into
//!                      paragraphs (the escape hatch for image-extraction
//!                      workflows, #173)
//!   --no-ocr           skip layout detection, OCR, and TableFormer entirely for
//!                      PDF/image input — no model load or inference at all.
//!                      Emits the embedded text layer as flat paragraphs in
//!                      reading order (no headings/lists/tables/pictures). The
//!                      fastest option, but a scanned/image-only PDF (no
//!                      embedded text layer) yields no text — convert those
//!                      without this flag.
//!   --use-web-browser  pre-render HTML/MHTML/EPUB in the system Chromium (driven
//!                      from Rust) so stylesheet-driven `display:none` elements
//!                      (e.g. a collapsed nav menu) are dropped before parsing.
//!                      Requires building with `--features web-browser`.
//!   --enrich-picture-classes
//!                      classify each detected picture (PDF/image input) with the
//!                      DocumentFigureClassifier model; the 26-class prediction
//!                      distribution lands in the JSON picture item (docling's
//!                      do_picture_classification). Needs
//!                      models/picture_classifier.onnx.
//!   --enrich-code      rewrite detected code blocks (and detect their language)
//!                      with the CodeFormulaV2 VLM (docling's do_code_enrichment).
//!                      Needs models/code_formula/. Slow on CPU: an autoregressive
//!                      generation per code block.
//!   --enrich-formula   decode display formulas to LaTeX with CodeFormulaV2
//!                      (docling's do_formula_enrichment); Markdown then renders
//!                      $$latex$$ instead of the formula placeholder comment.

use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use docling::{DocumentConverter, ImageMode, InputFormat, Pipeline, SourceDocument};

fn main() -> ExitCode {
    // `docling-rs serve …` — the HTTP conversion API (issue-#78 analogue of
    // docling-serve). Compiled in only with `--features serve`; the flags
    // after `serve` are the `docling-serve` binary's (see that crate).
    {
        let mut args = std::env::args().skip(1);
        if args.next().as_deref() == Some("serve") {
            return run_serve(args.collect());
        }
    }

    let mut strict = false;
    let mut to = "md".to_string();
    let mut images = "placeholder".to_string();
    let mut fetch_images = false;
    let mut no_stream = false;
    let mut no_table_former = false;
    let mut no_ocr = false;
    let mut force_full_page_ocr = false;
    let mut no_text_panels = false;
    let mut use_web_browser = false;
    let mut asr_model: Option<String> = None;
    let mut asr_lang: Option<String> = None;
    let mut video_frames: Option<usize> = None;
    let mut enrich_picture_classes = false;
    let mut enrich_code = false;
    let mut enrich_formula = false;
    let mut bench_warm: Option<usize> = None;
    let mut pages: Option<(usize, usize)> = None;
    let mut ocr_lang: Option<String> = None;
    let mut pipeline: Option<String> = None;
    let mut vlm_endpoint: Option<String> = None;
    let mut vlm_model: Option<String> = None;
    let mut path: Option<String> = None;
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut jobs: usize = 1;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--strict" => strict = true,
            "--fetch-images" => fetch_images = true,
            "--no-stream" => no_stream = true,
            "--no-table-former" => no_table_former = true,
            "--no-ocr" => no_ocr = true,
            "--force-full-page-ocr" => force_full_page_ocr = true,
            "--no-text-panels" => no_text_panels = true,
            "--use-web-browser" => use_web_browser = true,
            // Opt-in enrichment models (docling CLI flag names): picture
            // classification, code rewrite + language, formula LaTeX.
            "--enrich-picture-classes" => enrich_picture_classes = true,
            "--enrich-code" => enrich_code = true,
            "--enrich-formula" => enrich_formula = true,
            "--input" => match args.next() {
                Some(v) => input = Some(v),
                None => {
                    eprintln!("error: --input needs a glob pattern");
                    return ExitCode::from(2);
                }
            },
            "--output" => match args.next() {
                Some(v) => output = Some(v),
                None => {
                    eprintln!("error: --output needs a directory");
                    return ExitCode::from(2);
                }
            },
            "--jobs" => match args.next().and_then(|v| v.parse().ok()) {
                Some(n) if n >= 1 => jobs = n,
                _ => {
                    eprintln!("error: --jobs needs a positive integer");
                    return ExitCode::from(2);
                }
            },
            "--to" => to = args.next().unwrap_or_default(),
            // Named Whisper preset for audio inputs (English-only /
            // Distil-Whisper variants under models/asr/<preset>/; fetch with
            // download_dependencies.sh --asr-model=<preset>).
            "--asr-model" => asr_model = args.next(),
            // Transcription language (or "auto"); validated against the model's
            // vocabulary at conversion time.
            "--asr-lang" => asr_lang = args.next(),
            // Max frames sampled from a video input (needs the ffmpeg binary;
            // 0 = transcript only). Default 8.
            "--video-frames" => video_frames = args.next().and_then(|v| v.parse().ok()),
            "--images" => images = args.next().unwrap_or_default(),
            // PDF page window, 1-based inclusive: `--pages 3-7` or `--pages 3`.
            "--pages" => match args.next().as_deref().map(docling::parse_page_range) {
                Some(Ok(range)) => pages = Some(range),
                Some(Err(e)) => {
                    eprintln!("error: --pages: {e}");
                    return ExitCode::from(2);
                }
                None => {
                    eprintln!("error: --pages needs a range like 1-10 (or a single page)");
                    return ExitCode::from(2);
                }
            },
            // OCR recognition language for scanned PDF/image pages: en
            // (default; proper Latin word spacing) | ch (the multilingual
            // docling-conformance model).
            "--ocr-lang" => match args.next() {
                Some(v) if matches!(v.trim(), "en" | "ch") => ocr_lang = Some(v),
                Some(v) => {
                    eprintln!("error: --ocr-lang {v:?} is not en|ch");
                    return ExitCode::from(2);
                }
                None => {
                    eprintln!("error: --ocr-lang needs a value (en|ch)");
                    return ExitCode::from(2);
                }
            },
            // Pipeline selection (#77): `standard` (default, the ML stack) or
            // `vlm` — render pages and convert them through a remote
            // OpenAI-compatible vision endpoint returning DocLang.
            "--pipeline" => match args.next() {
                Some(v) if matches!(v.trim(), "standard" | "vlm") => pipeline = Some(v),
                Some(v) => {
                    eprintln!("error: --pipeline {v:?} is not standard|vlm");
                    return ExitCode::from(2);
                }
                None => {
                    eprintln!("error: --pipeline needs a value (standard|vlm)");
                    return ExitCode::from(2);
                }
            },
            "--vlm-endpoint" => vlm_endpoint = args.next(),
            "--vlm-model" => vlm_model = args.next(),
            // Hidden benchmarking aid: load the PDF/image pipeline once, then time
            // N warm conversions (models already loaded), printing the avg seconds
            // per conversion to stdout. This is the startup-excluded counterpart to
            // Python docling's in-process "warm" measurement, for a fair head-to-head.
            "--bench-warm" => {
                bench_warm = args.next().and_then(|n| n.parse::<usize>().ok());
                if bench_warm.is_none() {
                    eprintln!("error: --bench-warm needs a positive run count");
                    return ExitCode::from(2);
                }
            }
            _ if arg.starts_with("--") => {
                eprintln!(
                    "error: unknown flag '{arg}' (enrichment flags: --enrich-picture-classes, --enrich-code, --enrich-formula)"
                );
                return ExitCode::from(2);
            }
            _ => path = Some(arg),
        }
    }

    if !matches!(to.as_str(), "md" | "markdown" | "json" | "dclx" | "chunks") {
        eprintln!("error: unknown --to '{to}' (expected: md, json, dclx, chunks)");
        return ExitCode::from(2);
    }
    let image_mode = match images.as_str() {
        "placeholder" => ImageMode::Placeholder,
        "embedded" => ImageMode::Embedded,
        "referenced" => ImageMode::Referenced,
        other => {
            eprintln!(
                "error: unknown --images '{other}' (expected: placeholder, embedded, referenced)"
            );
            return ExitCode::from(2);
        }
    };

    // Batch mode (#205): `--input <glob>` fans one warm process over many
    // files, writing results under `--output` and preserving the directory
    // structure below the pattern's static prefix. A positional input file
    // with `--output` routes through the same writer (a batch of one).
    if input.is_some() || output.is_some() {
        if bench_warm.is_some() {
            eprintln!("error: --bench-warm is a single-file mode; drop --input/--output");
            return ExitCode::from(2);
        }
        let Some(outdir) = output else {
            eprintln!("error: --input needs --output DIR for the converted files");
            return ExitCode::from(2);
        };
        let (files, base) = if let Some(pattern) = &input {
            if path.is_some() {
                eprintln!("error: --input and a positional input file are mutually exclusive");
                return ExitCode::from(2);
            }
            match expand_glob(pattern) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::from(2);
                }
            }
        } else {
            let Some(p) = path else {
                eprintln!("error: --output needs --input GLOB or an input file");
                return ExitCode::from(2);
            };
            let file = std::path::PathBuf::from(&p);
            let base = file.parent().map(Path::to_path_buf).unwrap_or_default();
            (vec![file], base)
        };
        let vlm = if pipeline.as_deref() == Some("vlm") {
            match docling::vlm::VlmOptions::resolve(vlm_endpoint, vlm_model) {
                Ok(mut o) => {
                    o.page_range = pages;
                    Some(o)
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::from(2);
                }
            }
        } else {
            None
        };
        let cfg = BatchCfg {
            to,
            image_mode,
            strict,
            fetch_images,
            no_table_former,
            no_ocr,
            force_full_page_ocr,
            no_text_panels,
            use_web_browser,
            enrich_picture_classes,
            enrich_code,
            enrich_formula,
            asr_model,
            asr_lang,
            video_frames,
            pages,
            ocr_lang,
            vlm,
        };
        return run_batch(files, &base, Path::new(&outdir), jobs, &cfg);
    }

    let Some(path) = path else {
        eprintln!("usage: docling-rs [--strict] [--to md|json|dclx|chunks] [--images MODE] [--input GLOB --output DIR [--jobs N]] [--fetch-images] [--no-stream] [--no-table-former] [--no-ocr] [--force-full-page-ocr] [--no-text-panels] [--ocr-lang en|ch] [--use-web-browser] <input-file>");
        return ExitCode::from(2);
    };

    let source = match SourceDocument::from_file(&path) {
        Ok(src) => src,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(runs) = bench_warm {
        return match bench_warm_conversion(&source, runs, no_table_former, no_ocr) {
            Ok(avg) => {
                // Bare seconds on stdout for the benchmark harness; a human line on stderr.
                println!("{avg:.6}");
                eprintln!(
                    "warm conversion: {:.4}s/doc over {runs} runs (startup excluded)",
                    avg
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }

    // #77: the remote-VLM pipeline replaces the whole ML stack — convert,
    // then fall through to the regular output selection (md/json/dclx/chunks
    // all work; there is no page-streaming, the endpoint is the bottleneck).
    if pipeline.as_deref() == Some("vlm") {
        let mut opts = match docling::vlm::VlmOptions::resolve(vlm_endpoint, vlm_model) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        };
        opts.page_range = pages;
        let mut document = match docling::vlm::convert_vlm(&source, &opts) {
            Ok(doc) => doc,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        };
        document.strict_markdown = strict;
        return output_document(document, &to, image_mode, &path);
    }

    let mut converter = DocumentConverter::new()
        .strict(strict)
        .asr_model(asr_model.clone())
        .asr_lang(asr_lang.clone())
        .fetch_images(fetch_images)
        .no_table_former(no_table_former)
        .no_ocr(no_ocr)
        .force_full_page_ocr(force_full_page_ocr)
        .no_text_panels(no_text_panels)
        .use_web_browser(use_web_browser)
        .do_picture_classification(enrich_picture_classes)
        .do_code_enrichment(enrich_code)
        .do_formula_enrichment(enrich_formula);
    if let Some(max) = video_frames {
        converter = converter.video_frames(max);
    }
    if let Some((first, last)) = pages {
        converter = converter.page_range(first, last);
    }
    if let Some(lang) = &ocr_lang {
        converter = converter.ocr_lang(lang.clone());
    }

    // Stream Markdown by default: print each chunk as the converter produces it
    // (page by page for PDF). Referenced images stream too (#80): each page's
    // files land under ./artifacts/ as that page is printed, so image bytes
    // never accumulate. JSON needs the whole tree, so it keeps the buffered
    // path. `--no-stream` opts back into buffering.
    let is_markdown = matches!(to.as_str(), "md" | "markdown");
    if is_markdown && !no_stream {
        let stream = match converter.convert_streaming_images(source, image_mode) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        };
        let stdout = io::stdout();
        let mut out = io::BufWriter::new(stdout.lock());
        for chunk in stream {
            match chunk {
                Ok(s) => {
                    if let Err(e) = out.write_all(s.as_bytes()) {
                        eprintln!("error: writing output: {e}");
                        return ExitCode::FAILURE;
                    }
                }
                Err(e) => {
                    let _ = out.flush();
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        if let Err(e) = out.flush() {
            eprintln!("error: writing output: {e}");
            return ExitCode::FAILURE;
        }
        if image_mode == ImageMode::Referenced {
            eprintln!("referenced images (if any) written to ./artifacts/ as pages completed");
        }
        return ExitCode::SUCCESS;
    }

    let document = match converter.convert(source) {
        Ok(result) => result.document,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    output_document(document, &to, image_mode, &path)
}

/// The buffered output tail shared by the standard (non-streaming) and VLM
/// paths: `--to` selection, image sidecars, exit code.
/// The CLI flags a batch run freezes for every file (#205).
struct BatchCfg {
    to: String,
    image_mode: ImageMode,
    strict: bool,
    fetch_images: bool,
    no_table_former: bool,
    no_ocr: bool,
    force_full_page_ocr: bool,
    no_text_panels: bool,
    use_web_browser: bool,
    enrich_picture_classes: bool,
    enrich_code: bool,
    enrich_formula: bool,
    asr_model: Option<String>,
    asr_lang: Option<String>,
    video_frames: Option<usize>,
    pages: Option<(usize, usize)>,
    ocr_lang: Option<String>,
    vlm: Option<docling::vlm::VlmOptions>,
}

/// Expand an `--input` glob into (matched files, static base directory). The
/// base — every path component before the first one containing a glob
/// metacharacter — is what output paths are made relative to, so
/// `--input '/data/reports/**/*.pdf'` mirrors the tree under `/data/reports`
/// into `--output`.
fn expand_glob(pattern: &str) -> Result<(Vec<std::path::PathBuf>, std::path::PathBuf), String> {
    let base = glob_base(pattern);
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for entry in glob::glob(pattern).map_err(|e| format!("--input: {e}"))? {
        match entry {
            Ok(p) if p.is_file() => files.push(p),
            Ok(_) => {} // directories the pattern happens to match
            Err(e) => eprintln!("warning: {e}"),
        }
    }
    if files.is_empty() {
        return Err(format!("--input '{pattern}' matches no files"));
    }
    files.sort();
    Ok((files, base))
}

/// The static prefix of a glob pattern: every path component before the first
/// one containing a metacharacter. A metachar-free pattern is a literal file
/// path, whose base is its parent directory.
fn glob_base(pattern: &str) -> std::path::PathBuf {
    let mut base = std::path::PathBuf::new();
    for comp in Path::new(pattern).components() {
        let text = comp.as_os_str().to_string_lossy();
        if text.contains(['*', '?', '[']) {
            break;
        }
        base.push(comp);
    }
    if base == Path::new(pattern) {
        base.pop();
    }
    base
}

/// Where a converted file lands: `--output` + the input's path relative to the
/// glob base, with the extension swapped per `--to`.
fn batch_out_path(file: &Path, base: &Path, output: &Path, to: &str) -> std::path::PathBuf {
    let rel = file
        .strip_prefix(base)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| Path::new(file.file_name().unwrap_or_default()).to_path_buf());
    let ext = match to {
        "json" => "json",
        "dclx" => "dclx",
        "chunks" => "chunks.json",
        _ => "md",
    };
    output.join(rel).with_extension(ext)
}

/// Mirror of the single-file converter construction for batch workers.
fn batch_converter(cfg: &BatchCfg) -> DocumentConverter {
    let mut converter = DocumentConverter::new()
        .strict(cfg.strict)
        .asr_model(cfg.asr_model.clone())
        .asr_lang(cfg.asr_lang.clone())
        .fetch_images(cfg.fetch_images)
        .no_table_former(cfg.no_table_former)
        .no_ocr(cfg.no_ocr)
        .force_full_page_ocr(cfg.force_full_page_ocr)
        .no_text_panels(cfg.no_text_panels)
        .use_web_browser(cfg.use_web_browser)
        .do_picture_classification(cfg.enrich_picture_classes)
        .do_code_enrichment(cfg.enrich_code)
        .do_formula_enrichment(cfg.enrich_formula);
    if let Some(max) = cfg.video_frames {
        converter = converter.video_frames(max);
    }
    if let Some((first, last)) = cfg.pages {
        converter = converter.page_range(first, last);
    }
    if let Some(lang) = &cfg.ocr_lang {
        converter = converter.ocr_lang(lang.clone());
    }
    converter
}

/// The lazily-built warm PDF/image pipeline shared by every batch worker —
/// models load once and every subsequent PDF/image reuses the sessions, the
/// way docling-serve's warm pipeline does. Flags are frozen for the run, so
/// unlike serve there is nothing to rebuild per file.
fn batch_pipeline<'a>(
    slot: &'a mut Option<Pipeline>,
    cfg: &BatchCfg,
) -> Result<&'a mut Pipeline, String> {
    if slot.is_none() {
        let mut p = Pipeline::new()
            .map_err(|e| e.to_string())?
            .no_table_former(cfg.no_table_former)
            .no_ocr(cfg.no_ocr)
            .force_full_page_ocr(cfg.force_full_page_ocr)
            .no_text_panels(cfg.no_text_panels)
            .enrichments(docling::EnrichmentOptions {
                picture_classification: cfg.enrich_picture_classes,
                code: cfg.enrich_code,
                formula: cfg.enrich_formula,
            });
        p.set_pages(cfg.pages);
        p.set_ocr_lang(match cfg.ocr_lang.as_deref() {
            Some("ch") => Some(docling::OcrLang::Ch),
            Some(_) => Some(docling::OcrLang::En),
            None => None,
        });
        *slot = Some(p);
    }
    Ok(slot.as_mut().expect("just filled"))
}

/// Convert one batch file and write its output; returns the output path.
fn batch_convert_one(
    file: &Path,
    base: &Path,
    output: &Path,
    cfg: &BatchCfg,
    converter: &DocumentConverter,
    pipe: &std::sync::Mutex<Option<Pipeline>>,
) -> Result<std::path::PathBuf, String> {
    let source = SourceDocument::from_file(file).map_err(|e| e.to_string())?;
    let mut document = if let Some(vlm) = &cfg.vlm {
        docling::vlm::convert_vlm(&source, vlm).map_err(|e| e.to_string())?
    } else if matches!(source.format, InputFormat::Pdf | InputFormat::Image) {
        // One warm pipeline for the whole run: workers serialize on it (its
        // internal page workers already use the machine), declarative files
        // keep converting in parallel around it.
        let mut guard = pipe.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let p = batch_pipeline(&mut guard, cfg)?;
        match source.format {
            InputFormat::Pdf => p.convert(&source.bytes, None, &source.name),
            _ => p.convert_image(&source.bytes, &source.name),
        }
        .map_err(|e| e.to_string())?
    } else {
        converter
            .convert(source)
            .map_err(|e| e.to_string())?
            .document
    };
    document.strict_markdown = cfg.strict;

    let out = batch_out_path(file, base, output, &cfg.to);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    match cfg.to.as_str() {
        "json" => std::fs::write(&out, document.export_to_json())
            .map_err(|e| format!("writing {}: {e}", out.display()))?,
        "chunks" => std::fs::write(&out, chunks_json(&document))
            .map_err(|e| format!("writing {}: {e}", out.display()))?,
        "dclx" => docling::dclx::save_as_dclx(&document, &out).map_err(|e| e.to_string())?,
        _ => {
            if cfg.image_mode == ImageMode::Placeholder {
                std::fs::write(&out, document.export_to_markdown())
                    .map_err(|e| format!("writing {}: {e}", out.display()))?;
            } else {
                // `referenced` images land next to the output file, in a
                // per-document `<stem>_artifacts/` dir the links point into.
                let stem = out
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "document".into());
                let art = format!("{stem}_artifacts");
                let (md, artifacts) = document.export_to_markdown_with_images(cfg.image_mode, &art);
                let parent = out.parent().unwrap_or(Path::new(""));
                for (rel, bytes) in &artifacts {
                    let target = parent.join(rel);
                    if let Some(dir) = target.parent() {
                        std::fs::create_dir_all(dir)
                            .map_err(|e| format!("creating {}: {e}", dir.display()))?;
                    }
                    std::fs::write(&target, bytes)
                        .map_err(|e| format!("writing {}: {e}", target.display()))?;
                }
                std::fs::write(&out, md).map_err(|e| format!("writing {}: {e}", out.display()))?;
            }
        }
    }
    Ok(out)
}

/// Convert every matched file, `--jobs` workers wide. Output paths print to
/// stdout (one per line, for scripts); progress and errors go to stderr. A
/// failed file is reported and skipped — the batch keeps going, and the exit
/// code is non-zero if anything failed.
fn run_batch(
    files: Vec<std::path::PathBuf>,
    base: &Path,
    output: &Path,
    jobs: usize,
    cfg: &BatchCfg,
) -> ExitCode {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let pipe: std::sync::Mutex<Option<Pipeline>> = std::sync::Mutex::new(None);
    let workers = jobs.min(files.len()).max(1);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                // Converter construction is cheap configuration; one per
                // worker keeps the loop borrow-free.
                let converter = batch_converter(cfg);
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(file) = files.get(i) else { break };
                    match batch_convert_one(file, base, output, cfg, &converter, &pipe) {
                        Ok(out) => {
                            eprintln!("ok: {} -> {}", file.display(), out.display());
                            println!("{}", out.display());
                        }
                        Err(e) => {
                            failed.fetch_add(1, Ordering::Relaxed);
                            eprintln!("error: {}: {e}", file.display());
                        }
                    }
                }
            });
        }
    });
    let nf = failed.load(Ordering::Relaxed);
    eprintln!("batch: {} converted, {} failed", files.len() - nf, nf);
    if nf > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn output_document(
    document: docling::DoclingDocument,
    to: &str,
    image_mode: ImageMode,
    path: &str,
) -> ExitCode {
    if to == "json" {
        println!("{}", document.export_to_json());
        return ExitCode::SUCCESS;
    }

    if to == "chunks" {
        // Chunking conformance/debug dump: a JSON object with the hierarchical
        // chunk records and, when a tokenizer is configured, the hybrid ones.
        // `DOCLING_CHUNK_TOKENIZER` points at a HuggingFace tokenizer.json
        // (`DOCLING_CHUNK_MAX_TOKENS` overrides the default budget of 256).
        print!("{}", chunks_json(&document));
        return ExitCode::SUCCESS;
    }

    if to == "dclx" {
        // Binary OPC archive: written next to the CWD as `<input-stem>.dclx`
        // (stdout stays clean for terminals); the path is printed for scripts.
        let stem = Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "document".into());
        let out = std::path::PathBuf::from(format!("{stem}.dclx"));
        if let Err(e) = docling::dclx::save_as_dclx(&document, &out) {
            eprintln!("error: dclx: {e}");
            return ExitCode::FAILURE;
        }
        println!("{}", out.display());
        return ExitCode::SUCCESS;
    }

    if image_mode == ImageMode::Placeholder {
        print!("{}", document.export_to_markdown());
        return ExitCode::SUCCESS;
    }

    let (md, artifacts) = document.export_to_markdown_with_images(image_mode, "artifacts");
    for (rel, bytes) in &artifacts {
        let rel = Path::new(rel);
        if let Some(dir) = rel.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                eprintln!("error: creating {}: {e}", dir.display());
                return ExitCode::FAILURE;
            }
        }
        if let Err(e) = std::fs::write(rel, bytes) {
            eprintln!("error: writing {}: {e}", rel.display());
            return ExitCode::FAILURE;
        }
    }
    if !artifacts.is_empty() {
        eprintln!("wrote {} image(s) to ./artifacts/", artifacts.len());
    }
    print!("{md}");
    ExitCode::SUCCESS
}

/// `docling-rs serve …`: parse the serve flags and run the HTTP server.
#[cfg(feature = "serve")]
fn run_serve(args: Vec<String>) -> ExitCode {
    use docling_serve::ServeConfig;
    let mut cfg = ServeConfig::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--addr" => match it.next() {
                Some(v) => cfg.addr = v,
                None => return serve_usage("--addr needs HOST:PORT"),
            },
            "--concurrency" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) if v >= 1 => cfg.concurrency = v,
                _ => return serve_usage("--concurrency needs a positive integer"),
            },
            "--max-body-mb" => match it.next().and_then(|v| v.parse::<usize>().ok()) {
                Some(v) if v >= 1 => cfg.max_body_bytes = v * 1024 * 1024,
                _ => return serve_usage("--max-body-mb needs a positive integer"),
            },
            "--queue-size" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) if v >= 1 => cfg.queue_size = v,
                _ => return serve_usage("--queue-size needs a positive integer"),
            },
            "--result-ttl" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) if v >= 1 => cfg.result_ttl_secs = v,
                _ => return serve_usage("--result-ttl needs a positive number of seconds"),
            },
            "--warmup" => cfg.warmup = true,
            "--allow-url-fetch" => cfg.allow_url_fetch = true,
            "--no-url-fetch" => cfg.allow_url_fetch = false,
            "--strict" => cfg.strict = true,
            other => return serve_usage(&format!("unknown argument '{other}'")),
        }
    }
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(docling_serve::serve(cfg)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "serve")]
fn serve_usage(err: &str) -> ExitCode {
    eprintln!("error: {err}");
    eprintln!("usage: docling-rs serve [--addr HOST:PORT] [--concurrency N] [--max-body-mb N] [--queue-size N] [--result-ttl SECS] [--warmup] [--allow-url-fetch] [--strict]");
    ExitCode::from(2)
}

/// Without the `serve` feature the subcommand explains how to get it.
#[cfg(not(feature = "serve"))]
fn run_serve(_args: Vec<String>) -> ExitCode {
    eprintln!(
        "error: this binary was built without the HTTP server.\n\
         Rebuild with `cargo build -p docling-cli --features serve`, or use the\n\
         standalone server: `cargo run -p docling-serve --release -- --help`."
    );
    ExitCode::from(2)
}

/// Build the PDF/image pipeline once (loading the ONNX models), then time `runs`
/// warm conversions and return the average seconds per conversion. The first
/// conversion is a discarded warm-up that triggers the lazy model loads, so the
/// timed runs reuse them — the startup-excluded figure comparable to docling's
/// in-process warm number.
fn bench_warm_conversion(
    source: &SourceDocument,
    runs: usize,
    no_table_former: bool,
    no_ocr: bool,
) -> Result<f64, String> {
    let mut pipeline = Pipeline::new()
        .map_err(|e| e.to_string())?
        .no_table_former(no_table_former)
        .no_ocr(no_ocr);
    let once = |p: &mut Pipeline| -> Result<(), String> {
        match source.format {
            InputFormat::Pdf => p
                .convert(&source.bytes, None, &source.name)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            InputFormat::Image => p
                .convert_image(&source.bytes, &source.name)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            other => Err(format!(
                "--bench-warm supports PDF/image only, not {other:?}"
            )),
        }
    };
    once(&mut pipeline)?; // warm-up: load models, prime caches
    let mut total = 0.0f64;
    for _ in 0..runs {
        let t = std::time::Instant::now();
        once(&mut pipeline)?;
        total += t.elapsed().as_secs_f64();
    }
    Ok(total / runs as f64)
}

/// Serialize the chunk records `--to chunks` prints (see
/// [`docling::chunks::chunk_records`] for the tokenizer resolution rules).
fn chunks_json(document: &docling::DoclingDocument) -> String {
    let mut warn = |msg: String| eprintln!("warning: {msg}");
    let out = docling::chunks::chunk_records(document, &mut warn);
    format!(
        "{}\n",
        serde_json::to_string_pretty(&out).expect("chunks are serializable")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_base_stops_at_the_first_metachar_component() {
        assert_eq!(
            glob_base("/data/reports/**/*.pdf"),
            Path::new("/data/reports")
        );
        assert_eq!(glob_base("docs/*.md"), Path::new("docs"));
        assert_eq!(glob_base("*.md"), Path::new(""));
        assert_eq!(glob_base("a/b[12]/c/*.pdf"), Path::new("a"));
        // A literal file path (no metachars) bases at its parent, so a batch of
        // one lands directly under --output.
        assert_eq!(glob_base("dir/file.pdf"), Path::new("dir"));
    }

    #[test]
    fn batch_out_path_mirrors_structure_and_swaps_extension() {
        let out = |file: &str, to: &str| {
            batch_out_path(
                Path::new(file),
                Path::new("/data/reports"),
                Path::new("/out"),
                to,
            )
        };
        assert_eq!(
            out("/data/reports/a/b/x.pdf", "md"),
            Path::new("/out/a/b/x.md")
        );
        assert_eq!(out("/data/reports/x.pdf", "json"), Path::new("/out/x.json"));
        assert_eq!(out("/data/reports/x.pdf", "dclx"), Path::new("/out/x.dclx"));
        assert_eq!(
            out("/data/reports/a/x.pdf", "chunks"),
            Path::new("/out/a/x.chunks.json")
        );
        // A file outside the base still lands under --output by file name.
        assert_eq!(out("/elsewhere/y.docx", "md"), Path::new("/out/y.md"));
    }
}
