//! NVIDIA Parakeet TDT 0.6B v3 — the multilingual FastConformer transducer
//! (25 European languages, detected by the model itself) as an alternative
//! ASR backend to Whisper (#508): `asr_model = "parakeet_tdt_0.6b_v3"`.
//!
//! The ONNX export is `istupakov/parakeet-tdt-0.6b-v3-onnx` (CC-BY-4.0, from
//! NVIDIA's NeMo checkpoint): an encoder (`encoder-model[.int8].onnx`, 128
//! mel bands in, 1024-dim frames out at 8× subsampling = 80 ms per frame)
//! and a fused prediction-net + joint (`decoder_joint-model[.int8].onnx`).
//! Everything else is a port of onnx-asr 0.12, the reference runtime the
//! export is made for:
//!
//! * features — [`crate::nemo_mel`] (NeMo's preprocessor, numpy flavour);
//! * TDT greedy decoding (`NemoConformerTdt` + `_AsrWithTransducerDecoding`):
//!   per encoder frame the joint returns 8193 token logits (`<blk>` last) and
//!   5 duration logits (0–4 frames); a non-blank argmax token is emitted and
//!   the prediction-net state advances, the argmax duration moves the frame
//!   pointer, and a zero duration on a blank (or 10 tokens on one frame)
//!   steps one frame;
//! * text — SentencePiece pieces with `▁` as spaces, joined with onnx-asr's
//!   `\A\s|\s\B|(\s)\b` rule (a space survives only before a word
//!   character);
//! * long audio — Silero VAD ([`crate::vad`]) cuts speech into ≤ 20 s spans
//!   when its model is present (`.models/asr/vad/silero_vad.onnx`,
//!   `DOCLING_ASR_VAD_ONNX`; `DOCLING_RS_ASR_VAD=off` disables it); without it
//!   the audio is cut into ≤ 20 s pieces at the quietest 100 ms of each
//!   piece's last 5 s. Each span is encoded on its own, which keeps the
//!   encoder's full attention flat however long the recording is.
//!
//! Segments: within a span, a new segment starts after a token ending in
//! `.`, `?` or `!` that closes a word (NeMo's `segment_delimiter_tokens`), so
//! the output keeps docling's `[time: start-end] text` paragraphs sentence by
//! sentence. A segment starts at its first token's frame and ends at its last
//! token's frame plus that token's predicted duration (at least one frame),
//! offset by the span start and rounded to 10 ms like the Whisper path.
//!
//! The int8 graphs are the default (`encoder-model.int8.onnx` is 650 MB
//! where the fp32 one is 2.4 GB); `DOCLING_RS_FP32=1` — or a GPU execution
//! provider, as for the PDF models — prefers the fp32 files when present.

use ort::session::Session;
use ort::value::Tensor;

use crate::audio::SAMPLE_RATE;
use crate::nemo_mel;
use crate::vad::Vad;
use crate::whisper::Segment;

/// The Parakeet presets (each in its own `.models/asr/<preset>/`).
pub const PRESETS: &[&str] = &["parakeet_tdt_0.6b_v3"];
/// Encoder frames per second of audio (10 ms hop × 8× subsampling).
const FRAME_SECONDS: f64 = 0.08;
/// Duration classes of the TDT head (`[0, 1, 2, 3, 4]` frames).
const DURATIONS: usize = 5;
/// onnx-asr's `max_tokens_per_step` default.
const MAX_TOKENS_PER_STEP: usize = 10;
/// Upper bound of one encoder pass without VAD (and the VAD's own cap).
const MAX_CHUNK: usize = 20 * SAMPLE_RATE as usize;
/// Where the energy chunker looks for a cut: the last 5 s of a piece.
const CUT_SEARCH: usize = 5 * SAMPLE_RATE as usize;
/// Energy window of the chunker (100 ms) and its search step (10 ms).
const CUT_WINDOW: usize = SAMPLE_RATE as usize / 10;
const CUT_STEP: usize = SAMPLE_RATE as usize / 100;

/// Whether `preset` names a Parakeet model.
pub fn is_preset(preset: &str) -> bool {
    PRESETS.contains(&preset)
}

fn dir(preset: &str) -> String {
    format!(".models/asr/{preset}")
}

fn resolve(rel: &str) -> std::path::PathBuf {
    docling_core::assets::resolve(rel).into()
}

/// The ONNX graph to load for `stem` (`encoder-model`, `decoder_joint-model`):
/// the int8 export unless fp32 is preferred (`DOCLING_RS_FP32`, a GPU
/// provider), falling back to whichever of the two exists.
fn graph(preset: &str, stem: &str) -> Option<std::path::PathBuf> {
    let d = dir(preset);
    let int8 = resolve(&format!("{d}/{stem}.int8.onnx"));
    let fp32 = resolve(&format!("{d}/{stem}.onnx"));
    let fp32_first = docling_core::env::flag("DOCLING_RS_FP32") || docling_onnx::prefers_fp32();
    let order = if fp32_first {
        [fp32, int8]
    } else {
        [int8, fp32]
    };
    order.into_iter().find(|p| p.exists())
}

/// Whether the model files of `preset` are present.
pub fn models_available(preset: &str) -> bool {
    graph(preset, "encoder-model").is_some()
        && graph(preset, "decoder_joint-model").is_some()
        && resolve(&format!("{}/vocab.txt", dir(preset))).exists()
}

/// One emitted token: vocabulary id, encoder frame and predicted duration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Token {
    pub id: usize,
    pub frame: usize,
    pub dur: usize,
}

pub struct Parakeet {
    encoder: Session,
    decoder: Session,
    /// SentencePiece pieces by id (`▁` kept; `<blk>` last).
    vocab: Vec<String>,
    blank: usize,
    vad: Option<Vad>,
}

impl Parakeet {
    /// Load the encoder, the decoder-joint and the vocabulary of `preset`, and
    /// the Silero VAD when its model is present and not disabled.
    pub fn load(preset: &str) -> Result<Self, String> {
        let d = dir(preset);
        let missing = || {
            format!(
                "asr: Parakeet model files not found under {d}/ (run \
                 scripts/install/download_dependencies.sh --asr-model={preset})"
            )
        };
        let enc = graph(preset, "encoder-model").ok_or_else(missing)?;
        let dec = graph(preset, "decoder_joint-model").ok_or_else(missing)?;
        let vocab_path = resolve(&format!("{d}/vocab.txt"));
        let raw = std::fs::read_to_string(&vocab_path)
            .map_err(|e| format!("asr: reading {}: {e}", vocab_path.display()))?;
        let vocab = parse_vocab(&raw)?;
        let blank = vocab
            .iter()
            .position(|t| t == "<blk>")
            .ok_or_else(|| format!("asr: {} has no <blk> token", vocab_path.display()))?;
        docling_core::debug_log!(
            "docling-asr: parakeet: {} + {} ({} tokens)",
            enc.display(),
            dec.display(),
            vocab.len()
        );
        let encoder = crate::session(&enc)?;
        let decoder = crate::session(&dec)?;
        let vad = load_vad()?;
        Ok(Self {
            encoder,
            decoder,
            vocab,
            blank,
            vad,
        })
    }

    /// Transcribe 16 kHz mono samples into sentence segments.
    pub fn transcribe(&mut self, samples: &[f32]) -> Result<Vec<Segment>, String> {
        let spans = match self.vad.as_mut() {
            Some(vad) => vad.segments(samples)?,
            None => energy_chunks(samples),
        };
        docling_core::debug_log!(
            "docling-asr: parakeet: {} span(s) ({})",
            spans.len(),
            if self.vad.is_some() {
                "silero vad"
            } else {
                "energy chunks"
            }
        );
        let mut segments = Vec::new();
        for (start, end) in spans {
            let tokens = self.recognize(&samples[start..end])?;
            let offset = start as f64 / SAMPLE_RATE as f64;
            let limit = end as f64 / SAMPLE_RATE as f64;
            for (first, last) in sentences(&tokens, &self.vocab) {
                let text = decode_text(
                    tokens[first..=last]
                        .iter()
                        .map(|t| self.vocab[t.id].as_str()),
                )
                .trim()
                .to_string();
                if text.is_empty() {
                    continue;
                }
                let s = offset + tokens[first].frame as f64 * FRAME_SECONDS;
                let e =
                    offset + (tokens[last].frame + tokens[last].dur.max(1)) as f64 * FRAME_SECONDS;
                segments.push(Segment {
                    start: round2(s),
                    end: round2(e.min(limit).max(s)),
                    text,
                });
            }
        }
        Ok(segments)
    }

    /// Encode one span and run the TDT greedy decoder over it.
    pub(crate) fn recognize(&mut self, samples: &[f32]) -> Result<Vec<Token>, String> {
        let (features, frames, valid) = nemo_mel::features(samples);
        if valid < 2 {
            return Ok(Vec::new());
        }
        let feat = Tensor::from_array(([1usize, nemo_mel::N_MELS, frames], features))
            .map_err(|e| format!("asr: parakeet features: {e}"))?;
        let len = Tensor::from_array(([1usize], vec![valid as i64]))
            .map_err(|e| format!("asr: parakeet length: {e}"))?;
        let outputs = docling_onnx::run(
            &mut self.encoder,
            ort::inputs!["audio_signal" => feat, "length" => len],
        )
        .map_err(|e| format!("asr: parakeet encoder: {e}"))?;
        let (shape, enc) = outputs["outputs"]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("asr: parakeet encoder output: {e}"))?;
        let (_, lens) = outputs["encoded_lengths"]
            .try_extract_tensor::<i64>()
            .map_err(|e| format!("asr: parakeet encoder lengths: {e}"))?;
        // [1, D, T] → one D-vector per frame.
        let (dim, t_max) = (shape[1] as usize, shape[2] as usize);
        let enc_len = (lens[0].max(0) as usize).min(t_max);
        let enc = enc.to_vec();
        drop(outputs);
        let column = |t: usize| -> Vec<f32> { (0..dim).map(|d| enc[d * t_max + t]).collect() };

        let mut state = self.initial_state();
        let mut tokens: Vec<Token> = Vec::new();
        let (mut t, mut emitted) = (0usize, 0usize);
        while t < enc_len {
            let prev = tokens.last().map(|tok| tok.id).unwrap_or(self.blank);
            let (logits, durations, next_state) = self.joint(&column(t), prev, &state)?;
            let token = argmax(&logits);
            let step = argmax(&durations);
            if token != self.blank {
                state = next_state;
                tokens.push(Token {
                    id: token,
                    frame: t,
                    dur: step,
                });
                emitted += 1;
            }
            if step > 0 {
                t += step;
                emitted = 0;
            } else if token == self.blank || emitted == MAX_TOKENS_PER_STEP {
                t += 1;
                emitted = 0;
            }
        }
        Ok(tokens)
    }

    /// Zeroed LSTM states `[2, 1, 640]` ×2 — sized from the graph's inputs.
    fn initial_state(&self) -> [Vec<f32>; 2] {
        let size = |name: &str| -> usize {
            self.decoder
                .inputs()
                .iter()
                .find(|i| i.name() == name)
                .and_then(|i| i.dtype().tensor_shape().map(|s| s.to_vec()))
                .map(|dims| {
                    let layers = dims.first().copied().filter(|&d| d > 0).unwrap_or(2);
                    let hidden = dims.get(2).copied().filter(|&d| d > 0).unwrap_or(640);
                    (layers * hidden) as usize
                })
                .unwrap_or(2 * 640)
        };
        [
            vec![0f32; size("input_states_1")],
            vec![0f32; size("input_states_2")],
        ]
    }

    /// One prediction-net + joint step: token logits, duration logits and
    /// the state after feeding `prev`.
    #[allow(clippy::type_complexity)]
    fn joint(
        &mut self,
        frame: &[f32],
        prev: usize,
        state: &[Vec<f32>; 2],
    ) -> Result<(Vec<f32>, Vec<f32>, [Vec<f32>; 2]), String> {
        let hidden = |s: &Vec<f32>| s.len() / 2;
        let enc = Tensor::from_array(([1usize, frame.len(), 1], frame.to_vec()))
            .map_err(|e| format!("asr: parakeet joint input: {e}"))?;
        let targets = Tensor::from_array(([1usize, 1], vec![prev as i32]))
            .map_err(|e| format!("asr: parakeet targets: {e}"))?;
        let target_len = Tensor::from_array(([1usize], vec![1i32]))
            .map_err(|e| format!("asr: parakeet target length: {e}"))?;
        let s1 = Tensor::from_array(([2usize, 1, hidden(&state[0])], state[0].clone()))
            .map_err(|e| format!("asr: parakeet state 1: {e}"))?;
        let s2 = Tensor::from_array(([2usize, 1, hidden(&state[1])], state[1].clone()))
            .map_err(|e| format!("asr: parakeet state 2: {e}"))?;
        let outputs = docling_onnx::run(
            &mut self.decoder,
            ort::inputs![
                "encoder_outputs" => enc,
                "targets" => targets,
                "target_length" => target_len,
                "input_states_1" => s1,
                "input_states_2" => s2
            ],
        )
        .map_err(|e| format!("asr: parakeet joint: {e}"))?;
        let (_, out) = outputs["outputs"]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("asr: parakeet joint output: {e}"))?;
        let vocab = self.vocab.len();
        if out.len() < vocab + DURATIONS {
            return Err(format!(
                "asr: parakeet joint returned {} logits, expected {} + {DURATIONS}",
                out.len(),
                vocab
            ));
        }
        let logits = out[..vocab].to_vec();
        let durations = out[vocab..vocab + DURATIONS].to_vec();
        let (_, n1) = outputs["output_states_1"]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("asr: parakeet state 1 out: {e}"))?;
        let (_, n2) = outputs["output_states_2"]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("asr: parakeet state 2 out: {e}"))?;
        Ok((logits, durations, [n1.to_vec(), n2.to_vec()]))
    }
}

/// The Silero VAD session, unless disabled or its model is absent.
fn load_vad() -> Result<Option<Vad>, String> {
    if docling_core::env::nonempty("DOCLING_RS_ASR_VAD").is_some()
        && !docling_core::env::flag("DOCLING_RS_ASR_VAD")
    {
        return Ok(None);
    }
    let path: std::path::PathBuf = match docling_core::env::nonempty("DOCLING_ASR_VAD_ONNX") {
        Some(p) => p.into(),
        None => resolve(".models/asr/vad/silero_vad.onnx"),
    };
    if !path.exists() {
        docling_core::debug_log!(
            "docling-asr: parakeet: no VAD model at {} — energy-based chunking",
            path.display()
        );
        return Ok(None);
    }
    Ok(Some(Vad::new(crate::session(&path)?)))
}

/// `vocab.txt` (`<piece> <id>` per line) → pieces by id.
fn parse_vocab(raw: &str) -> Result<Vec<String>, String> {
    let mut pairs: Vec<(usize, String)> = Vec::new();
    for line in raw.lines().filter(|l| !l.is_empty()) {
        let (piece, id) = line
            .rsplit_once(' ')
            .ok_or_else(|| format!("asr: malformed vocab line '{line}'"))?;
        let id: usize = id
            .parse()
            .map_err(|_| format!("asr: malformed vocab id in '{line}'"))?;
        pairs.push((id, piece.to_string()));
    }
    pairs.sort_by_key(|(id, _)| *id);
    if pairs.iter().enumerate().any(|(i, (id, _))| i != *id) {
        return Err("asr: vocab ids are not contiguous from 0".to_string());
    }
    Ok(pairs.into_iter().map(|(_, piece)| piece).collect())
}

/// Join SentencePiece pieces into text with onnx-asr's
/// `re.sub(r"\A\s|\s\B|(\s)\b", …)`: `▁` becomes a space, and a whitespace
/// character survives (as one space) only when it is not the first character
/// and the next character is a word character — so no space before
/// punctuation, none leading.
pub(crate) fn decode_text<'a>(pieces: impl Iterator<Item = &'a str>) -> String {
    let joined: Vec<char> = pieces
        .flat_map(|p| p.chars())
        .map(|c| if c == '\u{2581}' { ' ' } else { c })
        .collect();
    let mut out = String::with_capacity(joined.len());
    for (i, &c) in joined.iter().enumerate() {
        if c.is_whitespace() {
            let next_is_word = joined.get(i + 1).is_some_and(|&n| is_word(n));
            if i > 0 && next_is_word {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Python `re`'s `\w` for `str` patterns: Unicode alphanumerics and `_`.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Sentence ranges `(first, last)` over the emitted tokens: a range closes
/// after a token whose text ends in `.`, `?` or `!` when the next token
/// starts a new word (or there is none).
pub(crate) fn sentences(tokens: &[Token], vocab: &[String]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut first = 0;
    for i in 0..tokens.len() {
        let piece = vocab[tokens[i].id].as_str();
        let closes = piece.trim_end().ends_with(['.', '?', '!']);
        let next_starts_word = tokens
            .get(i + 1)
            .is_none_or(|n| vocab[n.id].starts_with('\u{2581}'));
        if (closes && next_starts_word) || i + 1 == tokens.len() {
            out.push((first, i));
            first = i + 1;
        }
    }
    out
}

/// Without VAD: an energy-based stand-in. Pauses — runs of ≥ 300 ms whose
/// 100 ms energy stays in the bottom quarter of the recording's dynamic range
/// (between its 10th and 95th energy percentiles) — split the audio, spans
/// with no energy above that line are dropped, and a span still longer than
/// 20 s is cut at the quietest 100 ms of each 20 s piece's last 5 s. Pauses
/// matter beyond the encoder's cost: one span crossing a language switch
/// makes the transducer drop the second language's opening sentences, which
/// separate spans avoid. A recording without that much dynamic range (10 dB)
/// is only length-cut.
pub(crate) fn energy_chunks(samples: &[f32]) -> Vec<(usize, usize)> {
    let n = samples.len();
    if n <= CUT_WINDOW {
        return if n == 0 { Vec::new() } else { vec![(0, n)] };
    }
    // 100 ms energy in dB every 10 ms, from prefix sums of squares.
    let mut prefix = Vec::with_capacity(n + 1);
    prefix.push(0f64);
    for &v in samples {
        prefix.push(prefix.last().unwrap() + (v as f64) * (v as f64));
    }
    let db_at = |p: usize| {
        let e = (prefix[p + CUT_WINDOW] - prefix[p]) / CUT_WINDOW as f64;
        10.0 * (e + 1e-10).log10()
    };
    let positions: Vec<usize> = (0..=n - CUT_WINDOW).step_by(CUT_STEP).collect();
    let db: Vec<f64> = positions.iter().map(|&p| db_at(p)).collect();
    let mut sorted = db.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let pct = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
    let (floor, peak) = (pct(0.10), pct(0.95));

    let mut spans = Vec::new();
    if peak - floor < 10.0 {
        spans.push((0, n));
    } else {
        let line = floor + 0.25 * (peak - floor);
        let min_gap = 30; // 300 ms of 10 ms steps
        let mut cuts = vec![0usize];
        let mut i = 0;
        while i < db.len() {
            if db[i] < line {
                let mut j = i;
                while j < db.len() && db[j] < line {
                    j += 1;
                }
                if j - i >= min_gap {
                    let mid = positions[(i + j) / 2] + CUT_WINDOW / 2;
                    if mid > *cuts.last().unwrap() && mid < n {
                        cuts.push(mid);
                    }
                }
                i = j;
            } else {
                i += 1;
            }
        }
        cuts.push(n);
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            // Drop pure-pause spans: no 100 ms window above the line.
            let lo = a / CUT_STEP;
            let hi = (b.saturating_sub(CUT_WINDOW) / CUT_STEP).min(db.len().saturating_sub(1));
            if b > a && (lo..=hi.max(lo)).any(|k| db.get(k).is_some_and(|&d| d >= line)) {
                spans.push((a, b));
            }
        }
    }

    // Length cap at the quietest spot of each piece's last 5 s.
    let mut out = Vec::new();
    for (mut start, end) in spans {
        while end - start > MAX_CHUNK {
            let lo = start + MAX_CHUNK - CUT_SEARCH;
            let hi = start + MAX_CHUNK - CUT_WINDOW;
            let mut best = (f64::INFINITY, hi);
            let mut p = lo;
            while p <= hi {
                let e = prefix[p + CUT_WINDOW] - prefix[p];
                if e < best.0 {
                    best = (e, p);
                }
                p += CUT_STEP;
            }
            let cut = best.1 + CUT_WINDOW / 2;
            out.push((start, cut));
            start = cut;
        }
        if start < end {
            out.push((start, end));
        }
    }
    out
}

fn argmax(v: &[f32]) -> usize {
    let mut best = 0;
    for (i, &x) in v.iter().enumerate() {
        if x > v[best] {
            best = i;
        }
    }
    best
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab(pieces: &[&str]) -> Vec<String> {
        pieces.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn text_join_follows_the_reference_space_rule() {
        let pieces = ["▁Hello", ",", "▁wor", "ld", "▁!", "▁¿", "Qué", "?"];
        assert_eq!(decode_text(pieces.iter().copied()), "Hello, world!¿Qué?");
        // `\A\s` drops only the first space; the second precedes a word.
        assert_eq!(decode_text(["▁", "▁a"].iter().copied()), " a");
        assert_eq!(
            decode_text(["▁3", ".", "5", "▁km"].iter().copied()),
            "3.5 km"
        );
    }

    #[test]
    fn sentences_split_after_closing_punctuation_at_word_ends() {
        let v = vocab(&["▁Hi", ".", "▁e", "g", "▁Done", "?", "▁more"]);
        let tok = |id| Token {
            id,
            frame: 0,
            dur: 0,
        };
        // "Hi." | "e g Done?" | "more" — "." before "g" (no ▁) would not split.
        let toks: Vec<Token> = [0, 1, 2, 3, 4, 5, 6].iter().map(|&i| tok(i)).collect();
        assert_eq!(sentences(&toks, &v), vec![(0, 1), (2, 5), (6, 6)]);
        let toks: Vec<Token> = [2, 1, 3].iter().map(|&i| tok(i)).collect();
        assert_eq!(sentences(&toks, &v), vec![(0, 2)]);
    }

    #[test]
    fn vocab_parses_and_validates_ids() {
        let v = parse_vocab("<unk> 0\n▁the 1\n<blk> 2\n").unwrap();
        assert_eq!(v, vocab(&["<unk>", "▁the", "<blk>"]));
        assert!(parse_vocab("a 0\nb 2\n").is_err());
    }

    /// 45 s of continuous tone with two short dips: no pause is long enough
    /// to split on, so the 20 s length cap cuts at the dips.
    #[test]
    fn energy_chunks_cut_long_speech_at_the_quiet_spot() {
        let sr = SAMPLE_RATE as usize;
        let mut s: Vec<f32> = (0..45 * sr).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
        for q in [17 * sr, 35 * sr] {
            s[q..q + 2 * sr / 10].iter_mut().for_each(|v| *v = 0.001);
        }
        let chunks = energy_chunks(&s);
        assert_eq!(chunks.len(), 3, "{chunks:?}");
        assert!(
            (17 * sr..17 * sr + 2 * sr / 10).contains(&chunks[0].1),
            "{chunks:?}"
        );
        assert!(
            (35 * sr..35 * sr + 2 * sr / 10).contains(&chunks[1].1),
            "{chunks:?}"
        );
        assert_eq!(chunks[2].1, s.len());
        assert_eq!(energy_chunks(&s[..10 * sr]), vec![(0, 10 * sr)]);
    }

    /// Speech–pause–speech: the 1 s pause splits, the leading and trailing
    /// pauses are dropped.
    #[test]
    fn energy_chunks_split_on_pauses_and_drop_silence() {
        let sr = SAMPLE_RATE as usize;
        let tone = |len: usize| (0..len).map(|i| (i as f32 * 0.1).sin() * 0.5);
        let quiet = |len: usize| (0..len).map(|i| ((i * 7919) % 13) as f32 * 1e-4);
        let s: Vec<f32> = quiet(sr)
            .chain(tone(3 * sr))
            .chain(quiet(sr))
            .chain(tone(4 * sr))
            .chain(quiet(sr))
            .collect();
        let chunks = energy_chunks(&s);
        assert_eq!(chunks.len(), 2, "{chunks:?}");
        assert!(
            chunks[0].0 < sr && chunks[0].1 > 4 * sr && chunks[0].1 < 5 * sr,
            "{chunks:?}"
        );
        assert!(chunks[1].0 > 4 * sr && chunks[1].1 > 9 * sr, "{chunks:?}");
    }

    /// Point the asset resolution at the workspace-root `.models/` (tests run
    /// from the crate dir); `false` when the Parakeet export is not there —
    /// CI has no models, so the e2e tests below skip.
    fn models_ready() -> bool {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.models");
        std::env::set_var("DOCLING_RS_MODELS_DIR", &root);
        models_available(PRESETS[0])
    }

    fn fixture(name: &str) -> Vec<f32> {
        let path = format!(
            "{}/../../tests/data/audio/sources/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        crate::audio::decode_to_mono_16k(&bytes, name).expect("fixture decodes")
    }

    fn assert_well_formed(segments: &[Segment], duration: f64, what: &str) {
        assert!(!segments.is_empty(), "{what}: no segments");
        let mut prev_start = 0.0;
        for s in segments {
            assert!(!s.text.trim().is_empty(), "{what}: empty segment");
            assert!(s.start <= s.end, "{what}: {}–{} inverted", s.start, s.end);
            assert!(s.start >= prev_start, "{what}: segments out of order");
            assert!(
                s.end <= duration + 0.05,
                "{what}: {} past {duration}",
                s.end
            );
            prev_start = s.start;
        }
    }

    /// The segments' text, lowercased: the int8 and fp32 graphs (and ONNX
    /// Runtime builds) differ in casing and the odd word, so the checks
    /// look for phrases both variants agree on.
    fn joined(segments: &[Segment]) -> String {
        segments
            .iter()
            .map(|s| s.text.to_lowercase())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// #508 e2e (model-gated, one model load for all cases): English, German
    /// and Russian fixtures come back in their own language — the model picks
    /// it, no language option — as ordered sentence segments; a 40 s mix of
    /// the three is segmented by the VAD and, with the VAD off, by the energy
    /// chunker, landing every language in its own time range either way.
    #[test]
    fn transcribes_english_german_russian_and_long_mixes() {
        if !models_ready() {
            eprintln!(
                "skipping: Parakeet export missing under .models/asr/{}/",
                PRESETS[0]
            );
            return;
        }
        let mut p = Parakeet::load(PRESETS[0]).expect("models load");
        let cases = [
            ("sample_10s.mp3", "oscar wilde"),
            ("sample_14s_de.mp3", "selber nehmen"),
            ("sample_12s_ru.ogg", "первый урок"),
        ];
        let mut mix: Vec<f32> = Vec::new();
        let mut starts = Vec::new();
        for (name, phrase) in cases {
            let samples = fixture(name);
            let duration = samples.len() as f64 / SAMPLE_RATE as f64;
            let segments = p.transcribe(&samples).expect("transcribes");
            assert_well_formed(&segments, duration, name);
            assert!(
                joined(&segments).contains(phrase),
                "{name}: {}",
                joined(&segments)
            );
            starts.push(mix.len() as f64 / SAMPLE_RATE as f64);
            mix.extend_from_slice(&samples);
            // 1.5 s of room noise between the clips — not digital zeros, which
            // no recording has and which would swamp the per-span feature
            // normalization (ln(0 + 2⁻²⁴) ≈ −16.6) of a VAD-less span.
            let mut x = 0x2545_f491u32;
            mix.extend((0..3 * SAMPLE_RATE as usize / 2).map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x as f32 / u32::MAX as f32 - 0.5) * 0.004
            }));
        }
        let duration = mix.len() as f64 / SAMPLE_RATE as f64;
        assert!(duration > 2.0 * MAX_CHUNK as f64 / SAMPLE_RATE as f64 * 0.9);
        let with_vad = p.transcribe(&mix).expect("mix with VAD");
        let vad = p.vad.take();
        let without_vad = p.transcribe(&mix).expect("mix without VAD");
        p.vad = vad;
        for (segments, what) in [(&with_vad, "vad"), (&without_vad, "energy chunks")] {
            assert_well_formed(segments, duration, what);
            for (i, (_, phrase)) in cases.iter().enumerate() {
                let hit = segments
                    .iter()
                    .find(|s| s.text.to_lowercase().contains(phrase))
                    .unwrap_or_else(|| panic!("{what}: '{phrase}' missing: {}", joined(segments)));
                let end = starts.get(i + 1).copied().unwrap_or(duration);
                assert!(
                    hit.start >= starts[i] - 0.5 && hit.start < end,
                    "{what}: '{phrase}' at {} outside {}–{end}",
                    hit.start,
                    starts[i]
                );
            }
        }
    }

    /// The public entry point (model-gated): `[time: s-e] text` paragraphs,
    /// and an explicit language is ignored with a warning, not an error.
    #[test]
    fn convert_audio_routes_the_preset_and_ignores_asr_lang() {
        if !models_ready() {
            eprintln!(
                "skipping: Parakeet export missing under .models/asr/{}/",
                PRESETS[0]
            );
            return;
        }
        let path = format!(
            "{}/../../tests/data/audio/sources/sample_10s.mp3",
            env!("CARGO_MANIFEST_DIR")
        );
        let bytes = std::fs::read(path).expect("fixture");
        let doc = crate::convert_audio_with_options(
            &bytes,
            "sample_10s.mp3",
            Some(PRESETS[0]),
            Some("de"),
        )
        .expect("converts");
        let md = doc.export_to_markdown();
        assert!(md.starts_with("[time: "), "{md}");
        assert!(md.to_lowercase().contains("oscar wilde"), "{md}");
    }
}
