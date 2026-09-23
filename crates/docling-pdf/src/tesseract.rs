//! Tesseract as an alternative OCR engine (#460) — the system `tesseract`
//! binary driven like docling's `TesseractOcrCliModel`: a subprocess per
//! crop, `tsv` output on stdout, no bindings and no build-time dependency.
//! Selected with `DOCLING_RS_OCR_ENGINE=tesseract` / `--ocr-engine tesseract`;
//! PP-OCRv3 stays the default and the conformance engine.
//!
//! It plugs into the pipeline where the PP-OCR recognizer does — the same
//! layout-region crops, the same [`TextCell`] output in page points — so
//! everything downstream (orphan recovery, TableFormer word matching, the page
//! `ocr_score`) is engine-agnostic. Differences that follow from the engine:
//!
//! - Tesseract segments its own lines and words inside a region crop, so the
//!   projection-profile line splitter (`ocr_prep`) is not used; a region's
//!   cells are its `tsv` lines (words joined by single spaces), a table's
//!   cells its words — the granularity each consumer expects.
//! - Orientation (#225) comes from Tesseract's own OSD (`--psm 0 -l osd`,
//!   needs the `osd` traineddata) instead of the recognize-four-ways probe.
//! - Languages are tessdata stems (`eng`, `deu+fra`, `script/Cyrillic`), not
//!   the en/ch model switch — see [`lang_arg`] for what `ocr_lang` accepts
//!   under this engine.
//!
//! Every crop runs as its own `tesseract` process, dealt across the worker's
//! OCR lanes (`intra` threads, `DOCLING_RS_OCR_SESSIONS` overrides); the
//! child is pinned to one OpenMP thread so the lanes, not libgomp, own the
//! parallelism. Environment: `DOCLING_TESSERACT` (the binary; default
//! `tesseract` on `PATH`), `DOCLING_RS_TESSERACT_PSM` (page segmentation
//! mode 0–13, unset = Tesseract's default), `DOCLING_RS_TESSDATA_DIR`
//! (`--tessdata-dir`; `TESSDATA_PREFIX` is honored by Tesseract itself).
//! Runs are deterministic (Tesseract is; the results are placed by crop
//! index), so pinned outputs stay stable.

use std::io::Write;
use std::process::{Command, Stdio};

use image::{imageops, RgbImage};

use crate::layout::Region;
use crate::ocr_prep::is_text_label;
use crate::pdfium_backend::TextCell;
use docling_core::debug_log;

/// How the `tesseract` binary is invoked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TesseractOptions {
    /// Command or path of the executable (`DOCLING_TESSERACT`, default
    /// `tesseract`).
    pub cmd: String,
    /// The `-l` argument — tessdata stems joined with `+`, already mapped by
    /// [`lang_arg`]. `None` runs Tesseract's own default (`eng`).
    pub lang: Option<String>,
    /// Page segmentation mode (`--psm`, `DOCLING_RS_TESSERACT_PSM`, 0–13).
    /// `None` keeps Tesseract's default (3, fully automatic) — the right
    /// choice for layout-region crops; 6 (one uniform block) or 11 (sparse
    /// text) help on odd inputs.
    pub psm: Option<u8>,
    /// `--tessdata-dir` (`DOCLING_RS_TESSDATA_DIR`); `None` leaves the lookup
    /// to Tesseract (`TESSDATA_PREFIX`, then its build-time default).
    pub tessdata_dir: Option<String>,
}

impl TesseractOptions {
    /// The process-level options: `lang` from the caller (the mapped
    /// `ocr_lang`), the rest from the environment. A `DOCLING_RS_TESSERACT_PSM`
    /// outside 0–13 warns and is ignored.
    pub fn from_env(lang: Option<String>) -> Self {
        let psm = docling_core::env::nonempty("DOCLING_RS_TESSERACT_PSM").and_then(|raw| match raw
            .trim()
            .parse::<u8>()
        {
            Ok(n) if n <= 13 => Some(n),
            _ => {
                eprintln!(
                    "docling-pdf: DOCLING_RS_TESSERACT_PSM={raw:?} is not a page \
                         segmentation mode 0-13; using Tesseract's default"
                );
                None
            }
        });
        Self {
            cmd: docling_core::env::nonempty("DOCLING_TESSERACT")
                .unwrap_or_else(|| "tesseract".to_string()),
            lang,
            psm,
            tessdata_dir: docling_core::env::nonempty("DOCLING_RS_TESSDATA_DIR"),
        }
    }
}

/// Tesseract's `-l` argument for an `ocr_lang` value under this engine.
///
/// `+`-separated parts, each either a tessdata stem handed over verbatim —
/// `eng`, `chi_sim`, `srp_latn`, `script/Cyrillic`, a traineddata file of your
/// own (`[A-Za-z0-9_/][A-Za-z0-9_/-]*`, the identifier grammar docling's CLI
/// model sanitizes with) — or a BCP-47 tag mapped onto the stem Tesseract
/// names it by (its vocabulary is ISO 639-2/T: `de` → `deu`, `fr` → `fra`,
/// `zh-Hans` → `chi_sim`, `zh-TW` → `chi_tra`, `sr-Latn` → `srp_latn`).
/// A part is a tag when its primary subtag is two letters (docling's `iso:`
/// prefix is accepted and optional, as it is for the PP-OCR engine, #388);
/// region subtags are dropped, script subtags pick the variant. The PP-OCR
/// codes map too (`en` → `eng`, `ch` → `chi_sim`, docling's legacy
/// `english`/`chinese` and EasyOCR's `ch_sim`/`ch_tra`), so switching the
/// engine never invalidates an `ocr_lang` that worked. Multiple languages
/// are Tesseract's preference order. Unknown two-letter tags and malformed
/// stems are errors (the message says what to write instead); whether a stem
/// is *installed* is checked when the engine loads.
pub fn lang_arg(raw: &str) -> Result<String, String> {
    let mut stems = Vec::new();
    for part in raw.split('+') {
        let part = part.trim();
        let part = part
            .strip_prefix("iso:")
            .or_else(|| part.strip_prefix("ISO:"))
            .map_or(part, |tag| tag.trim());
        if part.is_empty() {
            return Err(format!("ocr_lang {raw:?} has an empty language entry"));
        }
        let mut subtags = part.split(['-', '_']);
        let primary = subtags.next().unwrap_or_default();
        let stem = if primary.len() == 2 && primary.bytes().all(|b| b.is_ascii_alphabetic()) {
            let rest: Vec<String> = subtags.map(str::to_ascii_lowercase).collect();
            bcp47_stem(&primary.to_ascii_lowercase(), &rest).ok_or_else(|| {
                format!(
                    "ocr_lang {part:?}: no Tesseract traineddata name is known for that \
                     BCP-47 tag; give the tessdata stem directly (e.g. `deu`, `chi_sim`, \
                     `script/Latin` — see `tesseract --list-langs`)"
                )
            })?
        } else {
            match part.to_ascii_lowercase().as_str() {
                "english" => "eng".to_string(),
                "chinese" => "chi_sim".to_string(),
                "chinese_cht" => "chi_tra".to_string(),
                _ => {
                    let mut chars = part.chars();
                    let head_ok = chars
                        .next()
                        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '/');
                    let tail_ok =
                        chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '-'));
                    if !(head_ok && tail_ok) {
                        return Err(format!(
                            "ocr_lang {part:?} is not a Tesseract language identifier \
                             (letters, digits, `_`, `/`, `-`; e.g. `eng`, `deu+fra`, \
                             `script/Cyrillic`)"
                        ));
                    }
                    part.to_string()
                }
            }
        };
        if !stems.contains(&stem) {
            stems.push(stem);
        }
    }
    Ok(stems.join("+"))
}

/// The tessdata stem for a two-letter primary subtag plus its remaining
/// subtags (lower-cased). Tesseract's names are ISO 639-2/T with a handful of
/// deviations (docling's `_TESSERACT_CANONICAL_TO_CODE_DEVIATIONS`); the
/// table covers the languages with a two-letter code among the traineddata
/// files Tesseract ships.
fn bcp47_stem(primary: &str, subtags: &[String]) -> Option<String> {
    let has = |s: &str| subtags.iter().any(|t| t == s);
    let stem = match primary {
        // Script-dependent stems.
        "zh" | "ch" => {
            if has("hant") || has("tw") || has("hk") || has("mo") || has("tra") {
                "chi_tra"
            } else {
                "chi_sim"
            }
        }
        "sr" => {
            if has("latn") {
                "srp_latn"
            } else {
                "srp"
            }
        }
        "az" => {
            if has("cyrl") {
                "aze_cyrl"
            } else {
                "aze"
            }
        }
        "uz" => {
            if has("cyrl") {
                "uzb_cyrl"
            } else {
                "uzb"
            }
        }
        "de" => {
            if has("latf") {
                "deu_latf"
            } else {
                "deu"
            }
        }
        "ku" => "kmr",
        "no" | "nb" | "nn" => "nor",
        "af" => "afr",
        "am" => "amh",
        "ar" => "ara",
        "as" => "asm",
        "be" => "bel",
        "bn" => "ben",
        "bo" => "bod",
        "bs" => "bos",
        "br" => "bre",
        "bg" => "bul",
        "ca" => "cat",
        "cs" => "ces",
        "co" => "cos",
        "cy" => "cym",
        "da" => "dan",
        "dv" => "div",
        "dz" => "dzo",
        "el" => "ell",
        "en" => "eng",
        "eo" => "epo",
        "et" => "est",
        "eu" => "eus",
        "fo" => "fao",
        "fa" => "fas",
        "fi" => "fin",
        "fr" => "fra",
        "fy" => "fry",
        "gd" => "gla",
        "ga" => "gle",
        "gl" => "glg",
        "gu" => "guj",
        "ht" => "hat",
        "he" => "heb",
        "hi" => "hin",
        "hr" => "hrv",
        "hu" => "hun",
        "hy" => "hye",
        "iu" => "iku",
        "id" => "ind",
        "is" => "isl",
        "it" => "ita",
        "jv" => "jav",
        "ja" => "jpn",
        "kn" => "kan",
        "ka" => "kat",
        "kk" => "kaz",
        "km" => "khm",
        "ky" => "kir",
        "ko" => "kor",
        "lo" => "lao",
        "la" => "lat",
        "lv" => "lav",
        "lt" => "lit",
        "lb" => "ltz",
        "ml" => "mal",
        "mr" => "mar",
        "mk" => "mkd",
        "mt" => "mlt",
        "mn" => "mon",
        "mi" => "mri",
        "ms" => "msa",
        "my" => "mya",
        "ne" => "nep",
        "nl" => "nld",
        "oc" => "oci",
        "or" => "ori",
        "pa" => "pan",
        "pl" => "pol",
        "pt" => "por",
        "ps" => "pus",
        "qu" => "que",
        "ro" => "ron",
        "ru" => "rus",
        "sa" => "san",
        "si" => "sin",
        "sk" => "slk",
        "sl" => "slv",
        "sd" => "snd",
        "es" => "spa",
        "sq" => "sqi",
        "su" => "sun",
        "sw" => "swa",
        "sv" => "swe",
        "ta" => "tam",
        "tt" => "tat",
        "te" => "tel",
        "tg" => "tgk",
        "th" => "tha",
        "ti" => "tir",
        "to" => "ton",
        "tr" => "tur",
        "ug" => "uig",
        "uk" => "ukr",
        "ur" => "urd",
        "vi" => "vie",
        "yi" => "yid",
        "yo" => "yor",
        _ => return None,
    };
    Some(stem.to_string())
}

/// One `tsv` word row (level 5): its position in Tesseract's block /
/// paragraph / line hierarchy, its box in crop pixels and its confidence
/// (0–100).
#[derive(Debug, Clone, PartialEq)]
struct Word {
    block: u32,
    par: u32,
    line: u32,
    l: f32,
    t: f32,
    r: f32,
    b: f32,
    conf: f32,
    text: String,
}

/// Parse Tesseract's `tsv` output: the header names the columns (`level
/// page_num block_num par_num line_num word_num left top width height conf
/// text`); only level-5 rows with non-blank text are words — the block /
/// paragraph / line rows carry no text and `conf -1`.
fn parse_tsv(tsv: &str) -> Vec<Word> {
    let mut lines = tsv.lines();
    let Some(header) = lines.next() else {
        return Vec::new();
    };
    let cols: Vec<&str> = header.split('\t').collect();
    let col = |name: &str| cols.iter().position(|c| *c == name);
    let (
        Some(level),
        Some(block),
        Some(par),
        Some(line),
        Some(left),
        Some(top),
        Some(w),
        Some(h),
        Some(conf),
        Some(text),
    ) = (
        col("level"),
        col("block_num"),
        col("par_num"),
        col("line_num"),
        col("left"),
        col("top"),
        col("width"),
        col("height"),
        col("conf"),
        col("text"),
    )
    else {
        return Vec::new();
    };
    let mut words = Vec::new();
    for row in lines {
        let f: Vec<&str> = row.split('\t').collect();
        if f.len() <= text || f[level] != "5" {
            continue;
        }
        let text = f[text].trim();
        if text.is_empty() {
            continue;
        }
        let num = |i: usize| f[i].trim().parse::<f32>().unwrap_or(0.0);
        let idx = |i: usize| f[i].trim().parse::<u32>().unwrap_or(0);
        let (l, t) = (num(left), num(top));
        words.push(Word {
            block: idx(block),
            par: idx(par),
            line: idx(line),
            l,
            t,
            r: l + num(w),
            b: t + num(h),
            conf: num(conf).clamp(0.0, 100.0),
            text: text.to_string(),
        });
    }
    words
}

/// A recognized text unit in crop pixels: `(l, t, r, b, text, confidence)`,
/// confidence on the 0–1 scale the page `ocr_score` expects.
type Unit = (f32, f32, f32, f32, String, f32);

/// Words → lines: consecutive words with the same block / paragraph / line
/// index join with single spaces (the order `tsv` lists them in is reading
/// order within the line), the box is their union and the confidence the
/// mean of theirs.
fn group_lines(words: &[Word]) -> Vec<Unit> {
    let mut out: Vec<Unit> = Vec::new();
    let mut key: Option<(u32, u32, u32)> = None;
    let mut n = 0.0f32;
    for w in words {
        let k = (w.block, w.par, w.line);
        if key == Some(k) {
            let last = out.last_mut().expect("a line is open");
            last.0 = last.0.min(w.l);
            last.1 = last.1.min(w.t);
            last.2 = last.2.max(w.r);
            last.3 = last.3.max(w.b);
            last.4.push(' ');
            last.4.push_str(&w.text);
            last.5 = (last.5 * n + w.conf / 100.0) / (n + 1.0);
            n += 1.0;
        } else {
            key = Some(k);
            n = 1.0;
            out.push((w.l, w.t, w.r, w.b, w.text.clone(), w.conf / 100.0));
        }
    }
    out
}

/// Each word on its own, for the table cell matcher.
fn word_units(words: &[Word]) -> Vec<Unit> {
    words
        .iter()
        .map(|w| (w.l, w.t, w.r, w.b, w.text.clone(), w.conf / 100.0))
        .collect()
}

/// Pixels of white paper added around a region crop: Tesseract's page
/// segmentation wants text clear of the image border, and a layout box
/// often sits on the ink.
const CROP_PAD: u32 = 6;

/// A region's crop as PNG bytes, with the crop's origin in image pixels.
fn crop_png(img: &RgbImage, region: &Region, scale: f32) -> Option<(u32, u32, Vec<u8>)> {
    let (iw, ih) = img.dimensions();
    let l = ((region.l * scale).max(0.0) as u32).saturating_sub(CROP_PAD);
    let t = ((region.t * scale).max(0.0) as u32).saturating_sub(CROP_PAD);
    let r = (((region.r * scale).max(0.0) as u32).saturating_add(CROP_PAD)).min(iw);
    let b = (((region.b * scale).max(0.0) as u32).saturating_add(CROP_PAD)).min(ih);
    if r <= l || b <= t {
        return None;
    }
    let crop = imageops::crop_imm(img, l, t, r - l, b - t).to_image();
    Some((l, t, encode_png(&crop)?))
}

fn encode_png(img: &RgbImage) -> Option<Vec<u8>> {
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).ok()?;
    Some(buf.into_inner())
}

/// The loaded engine: the validated options plus what `--list-langs` said.
pub struct TesseractOcr {
    opts: TesseractOptions,
    /// Crops recognized concurrently (one child process each).
    lanes: usize,
    /// Whether the `osd` traineddata is installed — orientation detection
    /// needs it.
    has_osd: bool,
}

impl TesseractOcr {
    /// Probe the binary (`--version`) and its languages (`--list-langs`),
    /// checking every requested stem is installed. An error here is what the
    /// pipeline shows as "OCR unavailable" (degrading like a missing model,
    /// #244) — it names the binary, the missing stems and what is installed.
    pub fn load(opts: TesseractOptions, lanes: usize) -> Result<Self, String> {
        let lanes = docling_core::env::parse::<usize>("DOCLING_RS_OCR_SESSIONS")
            .filter(|&n| n > 0)
            .unwrap_or(lanes)
            .clamp(1, 16);
        let version = Command::new(&opts.cmd)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| {
                format!(
                    "tesseract binary {:?} not runnable ({e}); install tesseract-ocr or point \
                     DOCLING_TESSERACT at it",
                    opts.cmd
                )
            })?;
        // Linux prints the version on stdout, older Windows builds on stderr.
        let banner = [version.stdout, version.stderr]
            .iter()
            .map(|b| String::from_utf8_lossy(b).trim().to_string())
            .find(|s| !s.is_empty())
            .unwrap_or_default();
        let banner = banner.lines().next().unwrap_or_default().to_string();
        let mut list = Command::new(&opts.cmd);
        list.arg("--list-langs").stdin(Stdio::null());
        if let Some(dir) = &opts.tessdata_dir {
            list.arg("--tessdata-dir").arg(dir);
        }
        let list = list
            .output()
            .map_err(|e| format!("tesseract --list-langs failed to run: {e}"))?;
        // The header line ("List of available languages in ... (N):") is
        // followed by one stem per line; Windows spells script packs with a
        // backslash, `-l` wants the slash everywhere.
        let installed: Vec<String> = String::from_utf8_lossy(&list.stdout)
            .lines()
            .skip(1)
            .map(|l| l.trim().replace('\\', "/"))
            .filter(|l| !l.is_empty())
            .collect();
        if installed.is_empty() {
            return Err(format!(
                "{banner}: no traineddata found (tesseract --list-langs printed nothing); \
                 install a language pack (e.g. tesseract-ocr-eng) or set \
                 DOCLING_RS_TESSDATA_DIR / TESSDATA_PREFIX"
            ));
        }
        let wanted: Vec<&str> = match &opts.lang {
            Some(l) => l.split('+').collect(),
            None => vec!["eng"],
        };
        let missing: Vec<&str> = wanted
            .iter()
            .copied()
            .filter(|w| !installed.iter().any(|i| i == w))
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "{banner}: no traineddata for {} (installed: {}); install the language \
                 pack or pick an installed one with ocr_lang",
                missing.join(", "),
                installed.join(", ")
            ));
        }
        let has_osd = installed.iter().any(|i| i == "osd");
        debug_log!(
            "docling-pdf: OCR engine {banner} (lang {}, psm {}, {} lane(s), osd {})",
            opts.lang.as_deref().unwrap_or("eng"),
            opts.psm.map_or("default".to_string(), |p| p.to_string()),
            lanes,
            if has_osd { "yes" } else { "no" }
        );
        Ok(Self {
            opts,
            lanes,
            has_osd,
        })
    }

    /// A child process with the common arguments: the binary, the language,
    /// the data directory, and one OpenMP thread (the lanes own the
    /// parallelism; libgomp's default of one thread per core per process
    /// would oversubscribe every core `lanes` times — and Tesseract is not
    /// faster multi-threaded on a small crop anyway).
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.opts.cmd);
        cmd.env("OMP_THREAD_LIMIT", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(dir) = &self.opts.tessdata_dir {
            cmd.arg("--tessdata-dir").arg(dir);
        }
        cmd
    }

    /// Feed `png` to a child on stdin and return its stdout. A child that
    /// fails (a crop Tesseract rejects) is an `Err` the caller degrades
    /// per crop.
    fn run(&self, mut cmd: Command, png: &[u8]) -> Result<String, String> {
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("tesseract: spawn {:?}: {e}", self.opts.cmd))?;
        // Write on a thread: a crop bigger than the pipe buffer would
        // otherwise deadlock against a child that has started printing.
        let mut stdin = child.stdin.take().expect("piped stdin");
        let png = png.to_vec();
        let writer = std::thread::spawn(move || stdin.write_all(&png));
        let out = child
            .wait_with_output()
            .map_err(|e| format!("tesseract: wait: {e}"))?;
        let _ = writer.join();
        if !out.status.success() {
            return Err(format!("tesseract exited with {}", out.status));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Recognize one crop: `tesseract stdin stdout [-l L] [--psm N] --dpi D tsv`.
    fn recognize(&self, png: &[u8], dpi: u32) -> Result<Vec<Word>, String> {
        let mut cmd = self.command();
        if let Some(lang) = &self.opts.lang {
            cmd.arg("-l").arg(lang);
        }
        if let Some(psm) = self.opts.psm {
            cmd.arg("--psm").arg(psm.to_string());
        }
        cmd.arg("--dpi").arg(dpi.to_string());
        cmd.args(["stdin", "stdout", "tsv"]);
        Ok(parse_tsv(&self.run(cmd, png)?))
    }

    /// Recognize every crop of `jobs` — `(origin l, origin t, png)` — across
    /// the lanes, and return each crop's units mapped to page points
    /// (`origin + px` / `scale`), in job order. A crop Tesseract fails on
    /// contributes nothing (logged under `DOCLING_RS_DEBUG`), the rest of the
    /// page still reads out.
    fn recognize_all(
        &self,
        jobs: &[(u32, u32, Vec<u8>)],
        scale: f32,
        units: fn(&[Word]) -> Vec<Unit>,
    ) -> Vec<(TextCell, f32)> {
        let dpi = (scale * 72.0).round().max(1.0) as u32;
        let lanes = self.lanes.min(jobs.len()).max(1);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let results: Vec<std::sync::Mutex<Option<Vec<Word>>>> =
            jobs.iter().map(|_| std::sync::Mutex::new(None)).collect();
        std::thread::scope(|s| {
            for _ in 0..lanes {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((_, _, png)) = jobs.get(i) else {
                        break;
                    };
                    let words = match self.recognize(png, dpi) {
                        Ok(words) => words,
                        Err(e) => {
                            debug_log!("docling-pdf: tesseract: crop {i}: {e}; no text");
                            Vec::new()
                        }
                    };
                    *results[i].lock().expect("crop result slot") = Some(words);
                });
            }
        });
        let mut cells = Vec::new();
        for ((ox, oy, _), slot) in jobs.iter().zip(results) {
            let words = slot
                .into_inner()
                .expect("crop result slot")
                .unwrap_or_default();
            for (l, t, r, b, text, conf) in units(&words) {
                let text = text.trim().to_string();
                if text.is_empty() {
                    continue;
                }
                cells.push((
                    TextCell {
                        text,
                        l: (*ox as f32 + l) / scale,
                        t: (*oy as f32 + t) / scale,
                        r: (*ox as f32 + r) / scale,
                        b: (*oy as f32 + b) / scale,
                    },
                    conf,
                ));
            }
        }
        cells
    }

    /// The crops of the regions `keep` selects.
    fn crops(
        img: &RgbImage,
        regions: &[Region],
        scale: f32,
        keep: fn(&str) -> bool,
    ) -> Vec<(u32, u32, Vec<u8>)> {
        regions
            .iter()
            .filter(|r| keep(r.label))
            .filter_map(|r| crop_png(img, r, scale))
            .collect()
    }

    /// OCR a page's text regions into line cells (page points) with their
    /// confidence — the counterpart of [`crate::ocr::OcrModel::ocr_page`].
    /// `scale` is image px per page point.
    pub fn ocr_page(
        &mut self,
        img: &RgbImage,
        regions: &[Region],
        scale: f32,
    ) -> Result<Vec<(TextCell, f32)>, String> {
        let jobs = crate::timing::timed("ocr.prep", || {
            Self::crops(img, regions, scale, is_text_label)
        });
        Ok(crate::timing::timed("ocr.rec", || {
            self.recognize_all(&jobs, scale, group_lines)
        }))
    }

    /// Word cells inside the page's table regions, for the cell matcher — the
    /// counterpart of [`crate::ocr::OcrModel::ocr_table_words`].
    pub fn ocr_table_words(
        &mut self,
        img: &RgbImage,
        regions: &[Region],
        scale: f32,
    ) -> Result<Vec<(TextCell, f32)>, String> {
        let jobs = Self::crops(img, regions, scale, crate::assemble::is_table_like);
        Ok(self.recognize_all(&jobs, scale, word_units))
    }

    /// The clockwise angle the page content is rotated by in `img`
    /// (`0`/`90`/`180`/`270`, the [`crate::orient::detect`] convention), from
    /// Tesseract's orientation-and-script detection (`--psm 0 -l osd`).
    /// `None` when the `osd` traineddata is not installed or OSD cannot make
    /// a call (too little text) — the page then stays as rendered.
    pub fn detect_orientation(&self, img: &RgbImage, scale: f32) -> Option<u16> {
        if !self.has_osd {
            debug_log!("docling-pdf: tesseract: no `osd` traineddata; orientation not probed");
            return None;
        }
        let png = encode_png(img)?;
        let mut cmd = self.command();
        cmd.args(["--psm", "0", "-l", "osd", "--dpi"])
            .arg(((scale * 72.0).round().max(1.0) as u32).to_string())
            .args(["stdin", "stdout"]);
        match self.run(cmd, &png) {
            Ok(out) => parse_osd(&out),
            Err(e) => {
                debug_log!("docling-pdf: tesseract OSD failed ({e}); assuming upright");
                None
            }
        }
    }
}

/// The `Orientation in degrees:` line of an OSD report — Tesseract's clockwise
/// angle of the text, the same convention as the probe's `deg` (its `Rotate:`
/// line is the complementary correction).
fn parse_osd(out: &str) -> Option<u16> {
    let deg = out
        .lines()
        .find_map(|l| l.trim().strip_prefix("Orientation in degrees:"))?
        .trim()
        .parse::<u16>()
        .ok()?;
    matches!(deg, 0 | 90 | 180 | 270).then_some(deg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ocr_lang` under the Tesseract engine: stems verbatim, BCP-47 tags and
    /// the PP-OCR codes mapped, lists joined in order, junk rejected.
    #[test]
    fn lang_arg_maps_tags_and_keeps_stems() {
        for (raw, want) in [
            ("eng", "eng"),
            ("en", "eng"),
            ("EN-us", "eng"),
            ("iso:en", "eng"),
            ("english", "eng"),
            ("ch", "chi_sim"),
            ("ch_tra", "chi_tra"),
            ("chinese_cht", "chi_tra"),
            ("zh", "chi_sim"),
            ("zh-Hant", "chi_tra"),
            ("zh-TW", "chi_tra"),
            ("iso:zh-CN", "chi_sim"),
            ("de", "deu"),
            ("de-Latf", "deu_latf"),
            ("sr-Latn", "srp_latn"),
            ("sr", "srp"),
            ("nb", "nor"),
            ("deu+fra", "deu+fra"),
            ("iso:de + fr", "deu+fra"),
            ("eng+eng", "eng"),
            ("script/Cyrillic", "script/Cyrillic"),
            ("chi_sim", "chi_sim"),
            ("srp_latn", "srp_latn"),
            ("custom_model", "custom_model"),
        ] {
            assert_eq!(lang_arg(raw).as_deref(), Ok(want), "{raw:?}");
        }
        for raw in ["", "xx", "en+", "de;u", "a b", "iso:", "deu fra"] {
            assert!(lang_arg(raw).is_err(), "{raw:?} should be rejected");
        }
    }

    /// The `tsv` shape Tesseract 5 prints: only level-5 rows are words; a
    /// block's two lines become two cells, words joined by single spaces,
    /// boxes unioned, confidence averaged onto 0–1; table mode keeps words.
    #[test]
    fn tsv_words_group_into_lines() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                   1\t1\t0\t0\t0\t0\t0\t0\t600\t160\t-1\t\n\
                   4\t1\t1\t1\t1\t0\t23\t28\t242\t25\t-1\t\n\
                   5\t1\t1\t1\t1\t1\t23\t28\t64\t20\t93.2\tHello\n\
                   5\t1\t1\t1\t1\t2\t94\t28\t93\t25\t92.4\tdocling\n\
                   5\t1\t1\t1\t1\t3\t195\t28\t70\t20\t96.0\tworld\n\
                   5\t1\t2\t1\t1\t1\t21\t98\t94\t20\t96.8\tSecond\n\
                   5\t1\t2\t1\t1\t2\t125\t98\t44\t20\t50\t \n\
                   5\t1\t2\t1\t1\t3\t179\t99\t44\t19\t96.8\t123\n";
        let words = parse_tsv(tsv);
        assert_eq!(words.len(), 5);
        let lines = group_lines(&words);
        assert_eq!(lines.len(), 2);
        let (l, t, r, b, text, conf) = &lines[0];
        assert_eq!(text, "Hello docling world");
        assert_eq!((*l, *t, *r, *b), (23.0, 28.0, 265.0, 53.0));
        assert!((conf - 0.9387).abs() < 1e-3, "{conf}");
        assert_eq!(lines[1].4, "Second 123");
        assert_eq!(word_units(&words).len(), 5);
        assert!(parse_tsv("").is_empty());
        assert!(parse_tsv("garbage\n5\t1\n").is_empty());
    }

    #[test]
    fn osd_orientation_parses() {
        let out = "Page number: 0\nOrientation in degrees: 270\nRotate: 90\n\
                   Orientation confidence: 6.47\nScript: Latin\nScript confidence: 4.05\n";
        assert_eq!(parse_osd(out), Some(270));
        assert_eq!(parse_osd("Orientation in degrees: 0\n"), Some(0));
        assert_eq!(parse_osd("Orientation in degrees: 45\n"), None);
        assert_eq!(parse_osd("Too few characters. Skipping this page\n"), None);
    }

    #[test]
    fn psm_env_is_range_checked() {
        let opts = TesseractOptions::from_env(Some("eng".into()));
        assert_eq!(opts.lang.as_deref(), Some("eng"));
        assert!(opts.psm.is_none() || opts.psm.is_some_and(|p| p <= 13));
    }
}
