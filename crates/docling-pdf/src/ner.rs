//! Named-entity detection for the PII redaction pass (#621): person,
//! organization and location spans from a token-classification ONNX model,
//! plugged into `docling_core::redact` through its `PiiDetector` trait.
//!
//! The model is a BERT-class token classifier with BIO labels
//! (`dslim/bert-base-NER`'s `O, B-MISC, I-MISC, B-PER, I-PER, B-ORG, I-ORG,
//! B-LOC, I-LOC` — MIT-licensed, the ONNX export HuggingFace ships), read
//! from `.models/ner/` (`DOCLING_RS_NER_DIR`): `model.onnx` (an
//! `model_int8.onnx` is preferred when present, like the other models),
//! `tokenizer.json` and `config.json` for the `id2label` map, so any model
//! with the same shape drops in. The session is opened through
//! `docling_onnx::session_builder()` like every other graph (execution
//! provider, thread budget, creation lock).
//!
//! A text is split into whitespace-bounded chunks that fit the model's 512
//! tokens, each chunk tokenized with offsets, and consecutive `B-X`/`I-X`
//! wordpieces merged into one span whose score is the mean of the tokens'
//! softmax probabilities; `MISC` is ignored (it is not personal data), and
//! `ADDRESS` is not a class this model has — the pass's `Address` kind stays
//! empty under it. The redaction pass applies its own score floor
//! (`RedactionOptions::ner_min_score`).

use std::sync::Mutex;

use docling_core::redact::{PiiDetector, PiiKind, Span};
use ort::session::Session;
use ort::value::Tensor;
use tokenizers::Tokenizer;

/// The model's context length (BERT); chunks stay well under it.
const MAX_TOKENS: usize = 512;
/// Characters per chunk: ~250 words, ~350 wordpieces of English prose.
const CHUNK_BYTES: usize = 1200;

pub struct NerDetector {
    /// `Session::run` is `&mut self`; the detector is shared behind a
    /// `PiiDetector` (`&self`) across a conversion.
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    /// `id2label` in index order.
    labels: Vec<String>,
    input_names: Vec<String>,
}

/// `DOCLING_RS_NER_DIR`, else `.models/ner` through the asset resolver.
pub fn model_dir() -> String {
    docling_core::env::nonempty("DOCLING_RS_NER_DIR")
        .unwrap_or_else(|| crate::resolve_asset(".models/ner"))
}

/// Whether the model files are installed (the test gate).
pub fn models_available() -> bool {
    let dir = model_dir();
    let p = |n: &str| std::path::Path::new(&dir).join(n).exists();
    (p("model.onnx") || p("model_int8.onnx")) && p("tokenizer.json") && p("config.json")
}

impl NerDetector {
    /// Load the model, tokenizer and label map. The error names what is
    /// missing — the caller warns once and continues pattern-only.
    pub fn load() -> Result<Self, String> {
        let dir = model_dir();
        let file = |n: &str| format!("{dir}/{n}");
        let int8 = file("model_int8.onnx");
        let model = if !crate::prefer_fp32() && std::path::Path::new(&int8).exists() {
            int8
        } else {
            file("model.onnx")
        };
        for f in [&model, &file("tokenizer.json"), &file("config.json")] {
            if !std::path::Path::new(f).exists() {
                return Err(format!("NER model file not found: {f}"));
            }
        }
        let config: serde_json::Value = serde_json::from_slice(
            &std::fs::read(file("config.json")).map_err(|e| format!("ner: config.json: {e}"))?,
        )
        .map_err(|e| format!("ner: config.json: {e}"))?;
        let id2label = config
            .get("id2label")
            .and_then(|v| v.as_object())
            .ok_or("ner: config.json has no id2label")?;
        let mut labels = vec![String::new(); id2label.len()];
        for (k, v) in id2label {
            let i: usize = k.parse().map_err(|_| format!("ner: id2label key {k:?}"))?;
            if i >= labels.len() {
                return Err(format!("ner: id2label index {i} out of range"));
            }
            labels[i] = v.as_str().unwrap_or("O").to_string();
        }
        let mut tokenizer = Tokenizer::from_file(file("tokenizer.json"))
            .map_err(|e| format!("ner: tokenizer: {e}"))?;
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: MAX_TOKENS,
                ..Default::default()
            }))
            .map_err(|e| format!("ner: tokenizer truncation: {e}"))?;
        let builder = docling_onnx::session_builder()?
            .with_intra_threads(crate::intra_threads())
            .map_err(|e| format!("ner: {e}"))?;
        let builder = docling_onnx::apply(builder)?;
        let session = docling_onnx::commit_uncached(builder, &model)
            .map_err(|e| format!("ner: load {model}: {e}"))?;
        let input_names = session
            .inputs()
            .iter()
            .map(|i| i.name().to_string())
            .collect();
        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
            labels,
            input_names,
        })
    }

    /// The spans of one chunk, as byte offsets into `chunk`.
    fn detect_chunk(&self, chunk: &str) -> Result<Vec<Span>, String> {
        let enc = self
            .tokenizer
            .encode(chunk, true)
            .map_err(|e| format!("ner: tokenize: {e}"))?;
        let ids: Vec<i64> = enc.get_ids().iter().map(|&v| v as i64).collect();
        let n = ids.len();
        if n == 0 {
            return Ok(Vec::new());
        }
        let mask: Vec<i64> = enc.get_attention_mask().iter().map(|&v| v as i64).collect();
        let types: Vec<i64> = enc.get_type_ids().iter().map(|&v| v as i64).collect();
        let offsets = enc.get_offsets();
        let specials = enc.get_special_tokens_mask();

        let logits: Vec<f32> = {
            let mut session = self.session.lock().unwrap_or_else(|p| p.into_inner());
            let mut inputs: Vec<(String, ort::value::DynValue)> = Vec::new();
            for name in &self.input_names {
                let data = match name.as_str() {
                    "input_ids" => ids.clone(),
                    "attention_mask" => mask.clone(),
                    "token_type_ids" => types.clone(),
                    other => return Err(format!("ner: unexpected model input {other:?}")),
                };
                let t = Tensor::from_array(([1usize, n], data))
                    .map_err(|e| format!("ner: input {name}: {e}"))?;
                inputs.push((name.clone(), t.into()));
            }
            let outputs = session.run(inputs).map_err(|e| format!("ner: run: {e}"))?;
            let (_, data) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("ner: output: {e}"))?;
            data.to_vec()
        };
        let classes = self.labels.len();
        if logits.len() != n * classes {
            return Err(format!(
                "ner: {} logits for {n} tokens × {classes} labels",
                logits.len()
            ));
        }

        // BIO merge over the wordpieces.
        let mut spans: Vec<Span> = Vec::new();
        let mut current: Option<(PiiKind, usize, usize, f32, usize)> = None; // kind, start, end, score sum, count
        let flush = |cur: &mut Option<(PiiKind, usize, usize, f32, usize)>, out: &mut Vec<Span>| {
            if let Some((kind, s, e, sum, cnt)) = cur.take() {
                if e > s {
                    out.push(Span {
                        start: s,
                        end: e,
                        kind,
                        score: sum / cnt as f32,
                        name: None,
                    });
                }
            }
        };
        for t in 0..n {
            if specials[t] != 0 {
                continue;
            }
            let row = &logits[t * classes..(t + 1) * classes];
            let (best, prob) = softmax_argmax(row);
            let label = &self.labels[best];
            let (prefix, kind) = match label.split_once('-') {
                Some((p, "PER")) => (p, Some(PiiKind::Person)),
                Some((p, "ORG")) => (p, Some(PiiKind::Organization)),
                Some((p, "LOC")) => (p, Some(PiiKind::Location)),
                _ => ("O", None),
            };
            let (ts, te) = offsets[t];
            match (kind, current.as_mut()) {
                (Some(k), Some(cur)) if prefix == "I" && cur.0 == k => {
                    cur.2 = te;
                    cur.3 += prob;
                    cur.4 += 1;
                }
                (Some(k), _) => {
                    flush(&mut current, &mut spans);
                    current = Some((k, ts, te, prob, 1));
                }
                (None, _) => flush(&mut current, &mut spans),
            }
        }
        flush(&mut current, &mut spans);
        Ok(spans)
    }
}

/// Argmax of a logit row and that class's softmax probability.
fn softmax_argmax(row: &[f32]) -> (usize, f32) {
    let max = row.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let sum: f32 = row.iter().map(|v| (v - max).exp()).sum();
    let (best, v) =
        row.iter().enumerate().fold(
            (0, f32::NEG_INFINITY),
            |acc, (i, &v)| if v > acc.1 { (i, v) } else { acc },
        );
    (best, (v - max).exp() / sum)
}

/// Whitespace-bounded chunks of at most `CHUNK_BYTES`, with their byte
/// offsets in `text`.
fn chunks(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + CHUNK_BYTES).min(text.len());
        if end < text.len() {
            // Back up to the last whitespace so a word is never split.
            match text[start..end].rfind(char::is_whitespace) {
                Some(i) if i > 0 => end = start + i,
                _ => {
                    while !text.is_char_boundary(end) {
                        end += 1;
                    }
                }
            }
        }
        out.push((start, &text[start..end]));
        start = end;
    }
    out
}

impl PiiDetector for NerDetector {
    fn detect(&self, text: &str) -> Vec<Span> {
        // Prose only: a string without a letter has no name in it, and the
        // model is the costly detector.
        if !text.chars().any(|c| c.is_alphabetic()) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (base, chunk) in chunks(text) {
            match self.detect_chunk(chunk) {
                Ok(spans) => out.extend(spans.into_iter().map(|mut s| {
                    s.start += base;
                    s.end += base;
                    s
                })),
                Err(e) => {
                    eprintln!("warning: NER detection failed: {e}");
                    break;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_split_on_whitespace_and_cover_the_text() {
        let word = "wordpiece ";
        let text: String = word.repeat(400);
        let parts = chunks(&text);
        assert!(parts.len() > 1);
        let joined: String = parts.iter().map(|(_, s)| *s).collect();
        assert_eq!(joined, text);
        for (base, s) in &parts {
            assert_eq!(&text[*base..*base + s.len()], *s);
            assert!(s.len() <= CHUNK_BYTES);
        }
        assert_eq!(chunks(""), Vec::<(usize, &str)>::new());
    }

    #[test]
    fn softmax_argmax_picks_the_top_class() {
        let (i, p) = softmax_argmax(&[0.0, 2.0, 1.0]);
        assert_eq!(i, 1);
        assert!(p > 0.6 && p < 0.7, "{p}");
    }

    /// With the model installed: a seeded name, organization and place are
    /// found with byte-exact spans (also past a multi-byte character).
    #[test]
    fn detects_seeded_entities() {
        if !models_available() {
            eprintln!("skipping: NER model not found");
            return;
        }
        let det = NerDetector::load().unwrap();
        let text = "Café note: Angela Merkel met Siemens AG in Berlin on Monday.";
        let spans = det.detect(text);
        let found: Vec<(PiiKind, &str)> = spans
            .iter()
            .map(|s| (s.kind, &text[s.start..s.end]))
            .collect();
        assert!(
            found.contains(&(PiiKind::Person, "Angela Merkel")),
            "{found:?}"
        );
        assert!(found.contains(&(PiiKind::Location, "Berlin")), "{found:?}");
        assert!(
            found
                .iter()
                .any(|(k, t)| *k == PiiKind::Organization && t.starts_with("Siemens")),
            "{found:?}"
        );
        assert!(spans.iter().all(|s| s.score > 0.5));
    }
}
