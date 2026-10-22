//! Silero VAD speech segmentation for the Parakeet path (#508) — a port of
//! onnx-asr's `SileroVad` + `BaseVad._merge_segments`, the reference runtime
//! of the Parakeet ONNX exports, with its defaults.
//!
//! The model (`silero_vad.onnx`, v5, MIT — `istupakov/silero-vad-onnx`, the
//! file onnx-asr loads) scores 32 ms hops (512 samples at 16 kHz), each fed
//! with the previous 64 samples as context and the recurrent state carried
//! across calls. Speech starts where the probability reaches 0.5 and ends
//! where it drops below 0.35; the raw spans are then merged and bounded:
//! gaps under 100 ms close, spans over 20 s are cut into 20 s pieces (the
//! encoder sees one piece at a time, which keeps its full-attention cost
//! flat however long the recording is), spans under 250 ms are dropped and
//! every span is padded by 30 ms.

use ort::session::Session;
use ort::value::Tensor;

use crate::audio::SAMPLE_RATE;

const HOP: usize = 512;
const CONTEXT: usize = 64;
const THRESHOLD: f32 = 0.5;
const NEG_THRESHOLD: f32 = THRESHOLD - 0.15;
const MIN_SPEECH_MS: i64 = 250;
const MAX_SPEECH_S: i64 = 20;
const MIN_SILENCE_MS: i64 = 100;
const SPEECH_PAD_MS: i64 = 30;

pub struct Vad {
    session: Session,
}

impl Vad {
    pub fn new(session: Session) -> Self {
        Self { session }
    }

    /// Speech spans of `samples` (16 kHz mono) as `(start, end)` sample
    /// offsets, merged and bounded as described in the module docs.
    pub fn segments(&mut self, samples: &[f32]) -> Result<Vec<(usize, usize)>, String> {
        let probs = self.probabilities(samples)?;
        let raw = find_segments(&probs);
        Ok(merge_segments(&raw, samples.len()))
    }

    /// One probability per 512-sample hop, in order (onnx-asr's `_encode`).
    fn probabilities(&mut self, samples: &[f32]) -> Result<Vec<f32>, String> {
        let n = samples.len();
        let mut state = vec![0f32; 2 * 128];
        let mut out = Vec::with_capacity(n / HOP + 2);
        let mut frame = vec![0f32; CONTEXT + HOP];

        // First hop: 64 zeros of context, then samples[..512].
        frame.iter_mut().for_each(|v| *v = 0.0);
        let first = n.min(HOP);
        frame[CONTEXT..CONTEXT + first].copy_from_slice(&samples[..first]);
        out.push(self.step(&frame[..CONTEXT + first], &mut state)?);

        // Full hops with the previous 64 samples as context.
        let mut j = HOP - CONTEXT;
        while j + CONTEXT + HOP <= n {
            out.push(self.step(&samples[j..j + CONTEXT + HOP], &mut state)?);
            j += HOP;
        }

        // The partial tail hop, zero-padded on the right.
        let last = n % HOP;
        if last != 0 && n > HOP {
            let tail = &samples[n - last - CONTEXT..];
            frame.iter_mut().for_each(|v| *v = 0.0);
            frame[..tail.len()].copy_from_slice(tail);
            out.push(self.step(&frame, &mut state)?);
        }
        Ok(out)
    }

    fn step(&mut self, frame: &[f32], state: &mut Vec<f32>) -> Result<f32, String> {
        let input = Tensor::from_array(([1usize, frame.len()], frame.to_vec()))
            .map_err(|e| format!("vad: input: {e}"))?;
        let st = Tensor::from_array(([2usize, 1, 128], std::mem::take(state)))
            .map_err(|e| format!("vad: state: {e}"))?;
        let sr = Tensor::from_array(((), vec![SAMPLE_RATE as i64]))
            .map_err(|e| format!("vad: sr: {e}"))?;
        let outputs = docling_onnx::run(
            &mut self.session,
            ort::inputs!["input" => input, "state" => st, "sr" => sr],
        )
        .map_err(|e| format!("vad: run: {e}"))?;
        let (_, p) = outputs["output"]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("vad: output: {e}"))?;
        let (_, s) = outputs["stateN"]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("vad: state out: {e}"))?;
        *state = s.to_vec();
        Ok(p[0])
    }
}

/// onnx-asr's `_find_segments`: hysteresis over the hop probabilities (a
/// trailing 0 closes an open span), in sample offsets.
pub(crate) fn find_segments(probs: &[f32]) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let mut speaking = false;
    let mut start = 0i64;
    for (i, &p) in probs.iter().chain(std::iter::once(&0.0)).enumerate() {
        let at = (i * HOP) as i64;
        if !speaking && p >= THRESHOLD {
            speaking = true;
            start = at;
        } else if speaking && p < NEG_THRESHOLD {
            speaking = false;
            out.push((start, at));
        }
    }
    out
}

/// onnx-asr's `BaseVad._merge_segments` with its default parameters,
/// returning `(start, end)` sample offsets clamped to the audio.
pub(crate) fn merge_segments(segments: &[(i64, i64)], len: usize) -> Vec<(usize, usize)> {
    const INF: i64 = 1_000_000_000_000_000;
    let sr = SAMPLE_RATE as i64;
    let len = len as i64;
    let pad = SPEECH_PAD_MS * sr / 1000;
    let min_speech = MIN_SPEECH_MS * sr / 1000 - 2 * pad;
    let max_speech = MAX_SPEECH_S * sr - 2 * pad;
    let min_silence = MIN_SILENCE_MS * sr / 1000 + 2 * pad;

    let mut out = Vec::new();
    let (mut cur_start, mut cur_end) = (-INF, -INF);
    let tail = [(len, len), (INF, INF)];
    for &(mut start, end) in segments.iter().chain(tail.iter()) {
        if start - cur_end < min_silence && end - cur_start < max_speech {
            cur_end = end;
        } else {
            if cur_start < len && cur_end > cur_start && cur_end - cur_start > min_speech {
                out.push((
                    (cur_start - pad).max(0) as usize,
                    (cur_end + pad).min(len) as usize,
                ));
            }
            while end - start > max_speech {
                out.push((
                    (start - pad).max(0) as usize,
                    (start + max_speech + pad) as usize,
                ));
                start += max_speech;
            }
            cur_start = start;
            cur_end = end;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hysteresis_opens_at_threshold_and_closes_below_the_negative_one() {
        // speech from hop 2 to hop 5 (0.4 stays inside, 0.3 closes).
        let probs = [0.1, 0.2, 0.6, 0.9, 0.4, 0.3, 0.1];
        assert_eq!(find_segments(&probs), vec![(2 * 512, 5 * 512)]);
        // an open span is closed by the implicit trailing 0.
        assert_eq!(find_segments(&[0.9, 0.9]), vec![(0, 2 * 512)]);
    }

    #[test]
    fn short_gaps_merge_and_spans_are_padded() {
        let sr = 16_000i64;
        // two 1 s spans 50 ms apart merge into one, padded by 30 ms (480).
        let segs = [(sr, 2 * sr), (2 * sr + 800, 3 * sr)];
        assert_eq!(
            merge_segments(&segs, 10 * sr as usize),
            vec![((sr - 480) as usize, (3 * sr + 480) as usize)]
        );
        // a 100 ms blip is dropped (under 250 ms).
        assert!(merge_segments(&[(sr, sr + 1600)], 10 * sr as usize).is_empty());
    }

    #[test]
    fn long_speech_is_cut_into_twenty_second_pieces() {
        let sr = 16_000i64;
        let len = 60 * sr as usize;
        let pieces = merge_segments(&[(0, 50 * sr)], len);
        assert!(pieces.len() >= 3, "{pieces:?}");
        for &(s, e) in &pieces {
            assert!(e - s <= 20 * sr as usize, "{s}..{e}");
        }
        assert_eq!(pieces.first().unwrap().0, 0);
        assert_eq!(pieces.last().unwrap().1, (50 * sr + 480) as usize);
    }
}
