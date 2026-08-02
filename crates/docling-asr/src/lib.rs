//! Audio/ASR backend for docling.rs — a port of docling's `AsrPipeline`
//! (native Whisper path, `temperature=0` greedy with timestamps).
//!
//! Everything runs in-process and in Rust: [`symphonia`] demuxes/decodes the
//! audio container (wav/mp3/flac/ogg/aac/m4a, plus the audio track of
//! mp4/mov/mkv/webm video — no ffmpeg), a ported log-mel
//! front-end feeds a **Whisper** encoder/decoder exported to ONNX (run on
//! [`ort`], like the PDF pipeline's layout/TableFormer/OCR models), and each
//! transcribed segment becomes one text paragraph in the docling conversation
//! form:
//!
//! ```text
//! [time: 2.0-7.72] And so my fellow Americans, ask not …
//! ```
//!
//! Model files (`encoder_model.onnx`, `decoder_model.onnx`, `vocab.json`, and
//! optionally `added_tokens.json` for non-English language selection) live in
//! `models/asr/` (override with `DOCLING_ASR_{ENCODER,DECODER,VOCAB}`) —
//! `scripts/install/download_dependencies.sh` fetches Whisper *tiny*, docling's ASR
//! default. The transcription language is auto-detected per file from the
//! first 30-second window (docling 2.116 parity); `DOCLING_RS_ASR_LANG` or
//! the `asr_lang` option pin it explicitly (`auto` re-enables detection).

pub mod audio;
pub mod mel;
pub mod tokenizer;
pub mod whisper;

use std::fmt;

use docling_core::{DoclingDocument, Node};

pub use whisper::{models_available, models_available_for, Segment, Transcriber, PRESETS};

/// Errors from the ASR backend. Detailed and surfaced (never silently skipped).
#[derive(Debug)]
pub struct AsrError(pub String);

impl fmt::Display for AsrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for AsrError {}

/// Convert an audio file (bytes + name, the extension hinting the container)
/// into a [`DoclingDocument`] of `[time: start-end] text` paragraphs.
///
/// Loads the Whisper models per call (the converter is one-shot); reuse a
/// [`Transcriber`] directly to batch many files. Fails with a clear message
/// when the model files are absent.
pub fn convert_audio(bytes: &[u8], name: &str) -> Result<DoclingDocument, AsrError> {
    convert_audio_with_model(bytes, name, None)
}

/// [`convert_audio`] with a named Whisper model preset (see [`PRESETS`]):
/// English-only and Distil-Whisper variants, each under its own
/// `models/asr/<preset>/` directory (docling PR #3741's presets, limited to
/// the variants with public ONNX exports).
pub fn convert_audio_with_model(
    bytes: &[u8],
    name: &str,
    model: Option<&str>,
) -> Result<DoclingDocument, AsrError> {
    convert_audio_with_options(bytes, name, model, None)
}

/// [`convert_audio_with_model`] with an explicit transcription language: a
/// Whisper code (`en`, `de`, `zh`, …) or `auto`. `None` falls back to
/// `DOCLING_RS_ASR_LANG`, and to per-file auto-detection when that is unset
/// too (multilingual presets; English-only ones always transcribe English).
pub fn convert_audio_with_options(
    bytes: &[u8],
    name: &str,
    model: Option<&str>,
    lang: Option<&str>,
) -> Result<DoclingDocument, AsrError> {
    let segments = transcribe_with_options(bytes, name, model, lang)?;
    let mut doc = DoclingDocument::new(name);
    for seg in segments {
        doc.nodes.push(Node::Paragraph {
            text: format!(
                "[time: {}-{}] {}",
                fmt_seconds(seg.start),
                fmt_seconds(seg.end),
                seg.text
            ),
        });
    }
    Ok(doc)
}

/// [`convert_audio_with_model`] up to (and excluding) document assembly: the
/// raw timed [`Segment`]s. The video pipeline (#138 Phase 2) uses this to
/// interleave sampled frames with the transcript by timestamp before building
/// the document.
pub fn transcribe_with_model(
    bytes: &[u8],
    name: &str,
    model: Option<&str>,
) -> Result<Vec<Segment>, AsrError> {
    transcribe_with_options(bytes, name, model, None)
}

/// [`transcribe_with_model`] with an explicit transcription language (see
/// [`convert_audio_with_options`] for the `None` fallback chain).
pub fn transcribe_with_options(
    bytes: &[u8],
    name: &str,
    model: Option<&str>,
    lang: Option<&str>,
) -> Result<Vec<Segment>, AsrError> {
    if !models_available_for(model) {
        let dir = match model {
            None | Some("whisper_tiny") | Some("") => "models/asr/".to_string(),
            Some(p) => format!("models/asr/{p}/"),
        };
        return Err(AsrError(format!(
            "asr: Whisper model files not found under {dir} \
             (run scripts/install/download_dependencies.sh{}, or set \
             DOCLING_ASR_{{ENCODER,DECODER,VOCAB}})",
            model
                .filter(|m| !m.is_empty() && *m != "whisper_tiny")
                .map(|m| format!(" --asr-model {m}"))
                .unwrap_or_default()
        )));
    }
    let samples = audio::decode_to_mono_16k(bytes, name).map_err(AsrError)?;
    let mut transcriber = Transcriber::load_preset(model).map_err(AsrError)?;
    if let Some(lang) = lang {
        transcriber.set_language(lang).map_err(AsrError)?;
    }
    transcriber.transcribe(&samples).map_err(AsrError)
}

/// Format seconds the way Python prints a rounded float (`0.0`, `7.5`, `7.72`)
/// — docling interpolates the values into `[time: {start}-{end}]` with plain
/// f-string formatting.
pub fn fmt_seconds(v: f64) -> String {
    let mut s = format!("{v}");
    if !s.contains('.') {
        s.push_str(".0");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_format_like_python_floats() {
        assert_eq!(fmt_seconds(0.0), "0.0");
        assert_eq!(fmt_seconds(7.5), "7.5");
        assert_eq!(fmt_seconds(7.72), "7.72");
        assert_eq!(fmt_seconds(30.0), "30.0");
    }

    /// Whether the Whisper models are reachable, pointing `DOCLING_ASR_*` at
    /// the workspace-root `models/asr/` (model resolution is CWD-relative and
    /// tests run from the crate dir).
    fn asr_models_ready() -> bool {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models/asr");
        if !root.join("encoder_model.onnx").exists() {
            return false;
        }
        std::env::set_var("DOCLING_ASR_ENCODER", root.join("encoder_model.onnx"));
        std::env::set_var("DOCLING_ASR_DECODER", root.join("decoder_model.onnx"));
        std::env::set_var("DOCLING_ASR_VOCAB", root.join("vocab.json"));
        std::env::set_var("DOCLING_ASR_ADDED_TOKENS", root.join("added_tokens.json"));
        true
    }

    fn fixture_bytes(name: &str) -> Vec<u8> {
        let path = format!(
            "{}/../../tests/data/audio/sources/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
    }

    /// #180 e2e (model-gated): on English speech, auto-detection picks `en`
    /// and the transcript is byte-identical to an explicit `en` request; an
    /// unknown language code is a clear error, not a silent fallback.
    #[test]
    fn auto_detected_language_matches_explicit_english() {
        if !asr_models_ready() {
            eprintln!("skipping: Whisper models missing under models/asr/");
            return;
        }
        let bytes = fixture_bytes("sample_10s.mp3");

        let auto = transcribe_with_options(&bytes, "sample_10s.mp3", None, Some("auto"))
            .expect("auto-detect transcribes");
        let en = transcribe_with_options(&bytes, "sample_10s.mp3", None, Some("en"))
            .expect("explicit en transcribes");
        let text = |segs: &[Segment]| {
            segs.iter()
                .map(|s| s.text.clone())
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(!auto.is_empty(), "transcript must not be empty");
        assert_eq!(text(&auto), text(&en), "auto-detect must resolve to en");

        let mut t = Transcriber::load_preset(None).expect("models load");
        let err = t.set_language("xx").expect_err("unknown code errors");
        assert!(err.contains("unknown language 'xx'"), "{err}");
        t.set_language("de").expect("known code is accepted");
        t.set_language("auto").expect("auto is accepted");
    }

    /// #180 e2e (model-gated): non-English speech resolves to the right
    /// language token — Russian over Ogg Vorbis, German over MP3 — and the
    /// transcript comes back non-empty in that language.
    #[test]
    fn auto_detects_russian_and_german() {
        if !asr_models_ready() {
            eprintln!("skipping: Whisper models missing under models/asr/");
            return;
        }
        for (fixture, expect) in [("sample_12s_ru.ogg", "ru"), ("sample_14s_de.mp3", "de")] {
            let bytes = fixture_bytes(fixture);
            let samples = audio::decode_to_mono_16k(&bytes, fixture).expect("fixture decodes");
            let mut t = Transcriber::load_preset(None).expect("models load");
            t.set_language("auto").expect("auto is accepted");
            let segments = t.transcribe(&samples).expect("transcribes");
            assert_eq!(t.detected_language(), Some(expect), "{fixture}");
            assert!(
                segments.iter().any(|s| !s.text.trim().is_empty()),
                "{fixture}: transcript must not be empty"
            );
        }
    }
}
