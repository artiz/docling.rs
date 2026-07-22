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
//! default. `DOCLING_RS_ASR_LANG` selects the transcription language (`en`).

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
    let segments = transcriber.transcribe(&samples).map_err(AsrError)?;

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

/// Format seconds the way Python prints a rounded float (`0.0`, `7.5`, `7.72`)
/// — docling interpolates the values into `[time: {start}-{end}]` with plain
/// f-string formatting.
fn fmt_seconds(v: f64) -> String {
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
}
