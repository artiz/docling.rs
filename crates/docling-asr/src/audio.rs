//! Audio decoding: any supported container/codec → 16 kHz mono `f32` samples.
//!
//! symphonia (pure Rust) demuxes and decodes every format docling's ASR accepts
//! — wav/pcm, mp3, flac, ogg/vorbis, aac (adts), m4a/mp4, plus the audio track
//! of video containers (mp4/mov via isomp4, mkv/webm via the Matroska reader)
//! — replacing the ffmpeg dependency Python docling shells out to. Channels are
//! averaged to mono and linearly resampled to Whisper's fixed 16 kHz input
//! rate.
//!
//! What symphonia can't handle in-process — Ogg **Opus** (no stable pure-Rust
//! decoder; the default codec of Telegram/WhatsApp voice messages) and **AVI**
//! containers (no demuxer) — falls back to the `ffmpeg` **binary** when one is
//! present (#190): runtime detection only, never a build dependency, same
//! pattern as video frame extraction (`DOCLING_FFMPEG` overrides the path).
//! Without ffmpeg the original targeted error is extended with an install
//! hint.

use symphonia::core::audio::AudioBufferRef;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Whisper's fixed input sample rate.
pub const SAMPLE_RATE: u32 = 16_000;

/// Decode `bytes` (using the file extension in `name` as a format hint) into
/// mono `f32` samples at 16 kHz. In-process symphonia first; whatever it
/// can't decode (Opus, AVI) goes through the optional ffmpeg fallback.
pub fn decode_to_mono_16k(bytes: &[u8], name: &str) -> Result<Vec<f32>, String> {
    let err = match decode_symphonia(bytes, name) {
        Ok(samples) => return Ok(samples),
        Err(e) => e,
    };
    match ffmpeg_binary() {
        Some(ffmpeg) => {
            eprintln!("docling.rs: asr: {err}; retrying with the ffmpeg fallback");
            decode_via_ffmpeg(&ffmpeg, bytes)
                .map_err(|fe| format!("{err}; ffmpeg fallback failed: {fe}"))
        }
        None => Err(format!(
            "{err} — install ffmpeg (or point DOCLING_FFMPEG at it) to enable the \
             fallback decoder for this input"
        )),
    }
}

/// The in-process pure-Rust decode path.
fn decode_symphonia(bytes: &[u8], name: &str) -> Result<Vec<f32>, String> {
    // symphonia has no AVI demuxer — its wav reader would reject the RIFF
    // header with an opaque "riff form is not wave"; say what's wrong instead.
    // Sniff the content (`RIFF....AVI `) rather than the name: callers often
    // pass a bare stem with no extension.
    let is_avi_bytes = bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"AVI ";
    let ext = name.rsplit('.').next().filter(|e| *e != name);
    if is_avi_bytes || ext.is_some_and(|e| e.eq_ignore_ascii_case("avi")) {
        return Err("asr: AVI containers are not supported (no pure-Rust demuxer)".to_string());
    }

    let cursor = std::io::Cursor::new(bytes.to_vec());
    let mss = MediaSourceStream::new(Box::new(cursor), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = ext {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("asr: unrecognized audio container: {e}"))?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| "asr: no decodable audio track".to_string())?;
    let track_id = track.id;
    // Clamp the header-declared sample rate to a sane range. `resample_linear`
    // upsamples by `src_rate / 16000`, and `Vec::with_capacity` on that factor
    // means a crafted file claiming `sample_rate = 1` would try to allocate
    // ~16000× the sample count → OOM abort. 8 kHz…768 kHz spans every real
    // audio rate.
    let src_rate = track
        .codec_params
        .sample_rate
        .unwrap_or(SAMPLE_RATE)
        .clamp(8_000, 768_000);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("asr: unsupported audio codec: {e}"))?;

    // Decode every packet, averaging channels to mono at the source rate. The
    // loop ends on any read error — end of stream, or a truncated tail (keep
    // what we decoded).
    let mut mono: Vec<f32> = Vec::new();
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // A corrupt frame is skipped, not fatal (matches ffmpeg's behavior).
            Err(_) => continue,
        };
        append_mono(&decoded, &mut mono);
    }

    if mono.is_empty() {
        return Err("asr: audio stream decoded to zero samples".to_string());
    }
    Ok(resample_linear(&mono, src_rate, SAMPLE_RATE))
}

/// The runnable ffmpeg binary, if any: `DOCLING_FFMPEG` if set, else `ffmpeg`
/// from `PATH` — probed per call, mirroring video-frame extraction.
fn ffmpeg_binary() -> Option<std::path::PathBuf> {
    let bin = std::env::var("DOCLING_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_string());
    std::process::Command::new(&bin)
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()
        .filter(|s| s.success())
        .map(|_| bin.into())
}

/// Decode arbitrary audio/video bytes to 16 kHz mono via the ffmpeg binary
/// (`-f s16le -ac 1 -ar 16000` on stdout), piping the input through stdin so
/// nothing touches the filesystem. stdin is fed from a separate thread while
/// stdout is drained here — writing 100 MB into a full pipe would deadlock a
/// single-threaded copy.
fn decode_via_ffmpeg(ffmpeg: &std::path::Path, bytes: &[u8]) -> Result<Vec<f32>, String> {
    use std::io::Read as _;
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut child = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i", "pipe:0"])
        .args(["-f", "s16le", "-ac", "1", "-ar", "16000", "pipe:1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawning {}: {e}", ffmpeg.display()))?;

    let mut stdin = child.stdin.take().expect("stdin piped");
    let input = bytes.to_vec();
    // The write end fails with EPIPE when ffmpeg rejects the input early;
    // that's fine — the exit status below carries the real diagnostic.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });

    let mut pcm = Vec::new();
    child
        .stdout
        .take()
        .expect("stdout piped")
        .read_to_end(&mut pcm)
        .map_err(|e| format!("reading ffmpeg output: {e}"))?;
    let mut diag = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut diag);
    }
    let status = child
        .wait()
        .map_err(|e| format!("waiting for ffmpeg: {e}"))?;
    let _ = writer.join();

    if !status.success() {
        return Err(format!("ffmpeg exited with {status}: {}", diag.trim()));
    }
    if pcm.len() < 2 {
        return Err("ffmpeg produced no audio".to_string());
    }
    Ok(pcm
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32_768.0)
        .collect())
}

/// Average all channels of a decoded buffer into `out` as `f32`.
fn append_mono(buf: &AudioBufferRef, out: &mut Vec<f32>) {
    macro_rules! mix {
        ($b:expr, $to_f32:expr) => {{
            let planes = $b.planes();
            let chans = planes.planes();
            if chans.is_empty() {
                return;
            }
            let frames = chans[0].len();
            let n = chans.len() as f32;
            for i in 0..frames {
                let mut acc = 0f32;
                for ch in chans {
                    acc += $to_f32(ch[i]);
                }
                out.push(acc / n);
            }
        }};
    }
    match buf {
        AudioBufferRef::F32(b) => mix!(b, |s: f32| s),
        AudioBufferRef::F64(b) => mix!(b, |s: f64| s as f32),
        AudioBufferRef::S32(b) => mix!(b, |s: i32| s as f32 / i32::MAX as f32),
        AudioBufferRef::S16(b) => mix!(b, |s: i16| s as f32 / i16::MAX as f32),
        AudioBufferRef::S8(b) => mix!(b, |s: i8| s as f32 / i8::MAX as f32),
        AudioBufferRef::U32(b) => mix!(b, |s: u32| (s as f32 / u32::MAX as f32) * 2.0 - 1.0),
        AudioBufferRef::U16(b) => mix!(b, |s: u16| (s as f32 / u16::MAX as f32) * 2.0 - 1.0),
        AudioBufferRef::U8(b) => mix!(b, |s: u8| (s as f32 / u8::MAX as f32) * 2.0 - 1.0),
        AudioBufferRef::S24(b) => mix!(b, |s: symphonia::core::sample::i24| s.inner() as f32
            / 8_388_607.0),
        AudioBufferRef::U24(b) => mix!(b, |s: symphonia::core::sample::u24| (s.inner() as f32
            / 16_777_215.0)
            * 2.0
            - 1.0),
    }
}

/// Linear-interpolation resampler. Whisper's mel front-end is robust to the
/// difference vs. a windowed-sinc resampler on speech, and this keeps the
/// pipeline dependency-free.
fn resample_linear(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let out_len = ((input.len() as f64) / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let i0 = pos.floor() as usize;
        let frac = (pos - i0 as f64) as f32;
        let a = input[i0];
        let b = *input.get(i0 + 1).unwrap_or(&a);
        out.push(a + (b - a) * frac);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_halves_and_keeps_length_ratio() {
        let input: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.01).sin()).collect();
        let out = resample_linear(&input, 32_000, 16_000);
        assert_eq!(out.len(), 500);
        // Downsampling by 2 keeps every other sample (linear interp at exact points).
        assert!((out[10] - input[20]).abs() < 1e-6);
    }

    /// Decode a fixture from the shared audio test-data tree, asserting the
    /// audio track demuxes to a duration in `[lo, hi]` seconds of non-silent
    /// speech at 16 kHz.
    fn decode_fixture_expect(name: &str, lo: f32, hi: f32) {
        let path = format!(
            "{}/../../tests/data/audio/sources/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let samples = decode_to_mono_16k(&bytes, name).expect("audio track decodes");
        let secs = samples.len() as f32 / SAMPLE_RATE as f32;
        assert!(
            (lo..=hi).contains(&secs),
            "{name}: expected {lo}-{hi}s of audio, got {secs:.2}s"
        );
        assert!(
            samples.iter().any(|&s| s.abs() > 0.01),
            "{name}: decoded audio is silent"
        );
    }

    fn decode_video_fixture(name: &str) {
        decode_fixture_expect(name, 8.0, 12.0);
    }

    // Video containers (#138 Phase 1): the audio track transcodes through the
    // same path as plain audio files; video frames are skipped by the
    // non-NULL-codec track selection.
    #[test]
    fn decodes_mp4_video_audio_track() {
        decode_video_fixture("sample_10s_video-mp4.mp4");
    }

    #[test]
    fn decodes_mov_video_audio_track() {
        decode_video_fixture("sample_10s_video-quicktime.mov");
    }

    #[test]
    fn decodes_mkv_video_audio_track() {
        decode_video_fixture("sample_10s_video-mkv.mkv");
    }

    #[test]
    fn decodes_webm_video_audio_track() {
        decode_video_fixture("sample_10s_video-webm.webm");
    }

    #[test]
    fn avi_gets_a_targeted_error() {
        // Garbage AVI bytes fail either way: without ffmpeg the targeted
        // symphonia-gap message (plus install hint), with ffmpeg the fallback
        // rejects the malformed input — "AVI" names the problem in both.
        // By extension (even with unreadable bytes)…
        let err = decode_to_mono_16k(&[0u8; 16], "clip.avi").unwrap_err();
        assert!(err.contains("AVI"), "unexpected error: {err}");
        // …and by content sniff when the name is a bare stem (the converter
        // passes `file_stem`, so the extension is not always available).
        let mut riff = b"RIFF\x00\x00\x00\x00AVI LIST".to_vec();
        riff.extend_from_slice(&[0u8; 32]);
        let err = decode_to_mono_16k(&riff, "clip_video-avi").unwrap_err();
        assert!(err.contains("AVI"), "unexpected error: {err}");
    }

    // #190: what symphonia can't decode goes through the ffmpeg binary when
    // one is present — Ogg Opus (no pure-Rust decoder)…
    #[test]
    fn opus_decodes_via_ffmpeg_fallback() {
        if ffmpeg_binary().is_none() {
            eprintln!("skipping: ffmpeg not available");
            return;
        }
        decode_fixture_expect("sample_12s_ru_opus.ogg", 11.0, 14.0);
    }

    // …and whole AVI containers (no pure-Rust demuxer).
    #[test]
    fn avi_decodes_via_ffmpeg_fallback() {
        if ffmpeg_binary().is_none() {
            eprintln!("skipping: ffmpeg not available");
            return;
        }
        decode_fixture_expect("sample_10s_video-avi.avi", 8.0, 12.0);
    }

    #[test]
    fn decodes_wav_bytes() {
        // Minimal 16-bit PCM wav: 100 samples of silence at 16 kHz.
        let mut wav = Vec::new();
        let data_len = 200u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&16_000u32.to_le_bytes());
        wav.extend_from_slice(&32_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        wav.extend(std::iter::repeat_n(0u8, data_len as usize));
        let samples = decode_to_mono_16k(&wav, "t.wav").expect("wav decodes");
        assert_eq!(samples.len(), 100);
        assert!(samples.iter().all(|&s| s == 0.0));
    }
}
