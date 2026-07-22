//! Video frame sampling for [`InputFormat::Video`](crate::InputFormat::Video)
//! (#138 Phase 2).
//!
//! Symphonia demuxes only the *audio* track of a video container; decoding the
//! video track (h264/vp8/vp9/…) has no mature pure-Rust story, so frames come
//! from the `ffmpeg` **binary** — detected at runtime, no build-time
//! dependency. Without ffmpeg on `PATH` (override with `DOCLING_FFMPEG`) a
//! video still converts exactly as in Phase 1: transcript only.
//!
//! Sampling strategy, per file:
//! 1. **Scene changes** — `select='eq(n,0)+gt(scene,0.27)'` keeps the first
//!    frame plus every cut sharper than the threshold, capped at `max_frames`.
//!    This favors slides/scene boundaries in screen recordings and lectures.
//! 2. **Uniform fallback** — a static or single-scene video yields just the
//!    first frame above, in which case `fps=max_frames/duration` resamples the
//!    timeline evenly.
//!
//! Each extracted frame carries the source timestamp (`showinfo`'s
//! `pts_time`); the converter interleaves frames with the ASR transcript by
//! time and each becomes a picture node captioned `[time: <ts>]`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use docling_core::{DoclingDocument, Node, PictureImage};

/// Convert a video: ASR transcript (Phase 1) interleaved by timestamp with up
/// to `max_frames` sampled frames as timestamped pictures (Phase 2).
///
/// Degradation is deliberate and asymmetric: a frame-extraction failure (or no
/// ffmpeg binary) never blocks the transcript, while a video with *no audio
/// track* (screen capture, muted clip) still converts to its frames alone.
/// Only "both sides empty" propagates the ASR error.
pub fn convert_video(
    bytes: &[u8],
    name: &str,
    asr_model: Option<&str>,
    max_frames: usize,
) -> Result<DoclingDocument, String> {
    let frames = if max_frames > 0 && ffmpeg_available() {
        match extract_frames(bytes, name, max_frames) {
            Ok(frames) => frames,
            Err(e) => {
                eprintln!("warning: {e}; continuing with the transcript only");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let segments = match docling_asr::transcribe_with_model(bytes, name, asr_model) {
        Ok(segments) => segments,
        Err(e) if !frames.is_empty() && e.0.contains("no decodable audio track") => Vec::new(),
        Err(e) => return Err(e.0),
    };

    let mut doc = DoclingDocument::new(name);
    let mut frames = frames.into_iter().peekable();
    for seg in &segments {
        // A frame sampled at (or before) the segment's start is what the
        // viewer sees as that speech begins — picture first.
        while frames.peek().is_some_and(|f| f.ts <= seg.start) {
            doc.nodes.push(picture_node(frames.next().unwrap()));
        }
        doc.nodes.push(Node::Paragraph {
            text: format!(
                "[time: {}-{}] {}",
                docling_asr::fmt_seconds(seg.start),
                docling_asr::fmt_seconds(seg.end),
                seg.text
            ),
        });
    }
    for frame in frames {
        doc.nodes.push(picture_node(frame));
    }
    Ok(doc)
}

/// A sampled frame as a picture node: `[time: <ts>]` caption (matching the
/// transcript's paragraph convention) and the PNG embedded for JSON/DCLX
/// export (Markdown renders its usual placeholder).
fn picture_node(frame: VideoFrame) -> Node {
    let (width, height) = png_size(&frame.png).unwrap_or((0, 0));
    Node::Picture {
        caption: Some(format!("[time: {}]", docling_asr::fmt_seconds(frame.ts))),
        image: Some(PictureImage {
            mimetype: "image/png".to_string(),
            width,
            height,
            data: frame.png,
        }),
        classification: None,
    }
}

/// Pixel size from a PNG IHDR (always the first chunk, at fixed offsets).
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    Some((be(&png[16..20]), be(&png[20..24])))
}

/// One sampled frame: PNG bytes plus the source-timeline timestamp in seconds.
pub struct VideoFrame {
    pub ts: f64,
    pub png: Vec<u8>,
}

/// The ffmpeg invocation to use: `DOCLING_FFMPEG` if set, else `ffmpeg` from
/// `PATH`.
fn ffmpeg_bin() -> String {
    std::env::var("DOCLING_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_string())
}

/// Whether the ffmpeg binary is runnable. Checked once per call site — the
/// converter probes before extracting and silently skips frames when absent.
pub fn ffmpeg_available() -> bool {
    Command::new(ffmpeg_bin())
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Sample up to `max_frames` frames from a video (bytes + name, the extension
/// hinting the container). `Ok(vec![])` means "no video track"; `Err` carries
/// the ffmpeg failure. Timestamps ascend.
pub fn extract_frames(
    bytes: &[u8],
    name: &str,
    max_frames: usize,
) -> Result<Vec<VideoFrame>, String> {
    if max_frames == 0 {
        return Ok(Vec::new());
    }
    let dir = TempDir::new()?;
    // ffmpeg needs a seekable input; piping via stdin breaks on mp4/mov files
    // whose moov atom trails the mdat. Keep the caller's extension as the
    // container hint (content sniffing still decides).
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin");
    let input = dir.path.join(format!("input.{ext}"));
    std::fs::File::create(&input)
        .and_then(|mut f| f.write_all(bytes))
        .map_err(|e| format!("video: writing temp file: {e}"))?;

    let probe = probe(&input)?;
    if !probe.has_video {
        return Ok(Vec::new());
    }

    // Pass 1: scene changes (always includes frame 0).
    let mut frames = run_filter(
        &dir,
        &input,
        "select='eq(n,0)+gt(scene,0.27)',showinfo",
        max_frames,
        "scene",
    )?;
    // Pass 2: a single kept frame means no cuts were found — resample evenly.
    if frames.len() < 2 && max_frames >= 2 {
        if let Some(duration) = probe.duration {
            if duration > 0.0 {
                frames = run_filter(
                    &dir,
                    &input,
                    &format!("fps={max_frames}/{duration},showinfo"),
                    max_frames,
                    "uniform",
                )?;
            }
        }
    }
    Ok(frames)
}

struct Probe {
    has_video: bool,
    duration: Option<f64>,
}

/// `ffmpeg -i <file>` with no output exits non-zero but prints the stream
/// layout; parse `Duration:` and whether a `Video:` stream exists.
fn probe(input: &Path) -> Result<Probe, String> {
    let out = Command::new(ffmpeg_bin())
        .args(["-hide_banner", "-i"])
        .arg(input)
        .output()
        .map_err(|e| format!("video: running ffmpeg: {e}"))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let has_video = stderr
        .lines()
        .any(|l| l.trim_start().starts_with("Stream #") && l.contains("Video:"));
    let duration = stderr.lines().find_map(|l| {
        let rest = l.trim_start().strip_prefix("Duration: ")?;
        let hms = rest.split(',').next()?.trim();
        let mut parts = hms.split(':');
        let h: f64 = parts.next()?.parse().ok()?;
        let m: f64 = parts.next()?.parse().ok()?;
        let s: f64 = parts.next()?.parse().ok()?;
        Some(h * 3600.0 + m * 60.0 + s)
    });
    Ok(Probe {
        has_video,
        duration,
    })
}

/// Run one extraction pass: `-vf <filter>` (which must end in `showinfo`),
/// writing at most `max_frames` PNGs, and zip the emitted files with the
/// source timestamps `showinfo` logs (`pts_time:<seconds>`).
fn run_filter(
    dir: &TempDir,
    input: &Path,
    filter: &str,
    max_frames: usize,
    tag: &str,
) -> Result<Vec<VideoFrame>, String> {
    let pattern = dir.path.join(format!("{tag}_%04d.png"));
    let out = Command::new(ffmpeg_bin())
        .args(["-hide_banner", "-i"])
        .arg(input)
        .args(["-vf", filter, "-fps_mode", "vfr", "-frames:v"])
        .arg(max_frames.to_string())
        .args(["-f", "image2", "-y"])
        .arg(&pattern)
        .output()
        .map_err(|e| format!("video: running ffmpeg: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let last = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        return Err(format!("video: ffmpeg frame extraction failed: {last}"));
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let timestamps: Vec<f64> = stderr
        .lines()
        .filter(|l| l.contains("Parsed_showinfo"))
        .filter_map(|l| {
            let rest = l.split("pts_time:").nth(1)?;
            rest.split_whitespace().next()?.parse().ok()
        })
        .collect();
    let mut frames = Vec::new();
    for i in 1..=max_frames {
        let path = dir.path.join(format!("{tag}_{i:04}.png"));
        let Ok(png) = std::fs::read(&path) else { break };
        // showinfo logs one line per selected frame, in emit order; a missing
        // timestamp (log truncation) falls back to 0 rather than dropping the
        // frame.
        let ts = timestamps.get(i - 1).copied().unwrap_or(0.0);
        frames.push(VideoFrame { ts, png });
    }
    Ok(frames)
}

/// Minimal scoped temp dir (no `tempfile` dependency): unique per process ×
/// counter, best-effort removed on drop.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Result<Self, String> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "docling-video-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).map_err(|e| format!("video: creating temp dir: {e}"))?;
        Ok(Self { path })
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/data/audio/sources")
            .join(name);
        std::fs::read(path).expect("fixture exists")
    }

    /// PNG magic on every frame, ascending timestamps, count within cap.
    fn assert_frames(frames: &[VideoFrame], max: usize) {
        assert!(!frames.is_empty(), "no frames extracted");
        assert!(frames.len() <= max);
        for w in frames.windows(2) {
            assert!(w[0].ts <= w[1].ts, "timestamps not ascending");
        }
        for f in frames {
            assert_eq!(&f.png[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
        }
    }

    #[test]
    fn extracts_frames_from_mp4() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not on PATH");
            return;
        }
        let frames = extract_frames(&fixture("sample_10s_video-mp4.mp4"), "s.mp4", 8).unwrap();
        assert_frames(&frames, 8);
        // The fixture is a single continuous scene → uniform sampling kicks in
        // and spreads frames across the ~10 s timeline.
        assert!(
            frames.len() >= 4,
            "expected uniform sampling, got {}",
            frames.len()
        );
        assert!(frames.last().unwrap().ts > 5.0);
    }

    #[test]
    fn extracts_frames_from_webm() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not on PATH");
            return;
        }
        let frames = extract_frames(&fixture("sample_10s_video-webm.webm"), "s.webm", 4).unwrap();
        assert_frames(&frames, 4);
    }

    #[test]
    fn audio_only_yields_no_frames() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not on PATH");
            return;
        }
        let frames = extract_frames(&fixture("sample_10s_audio-mp3.mp3"), "s.mp3", 8).unwrap();
        assert!(frames.is_empty(), "audio file must yield no frames");
    }

    #[test]
    fn zero_max_frames_disables_extraction() {
        let frames = extract_frames(b"not a video", "s.mp4", 0).unwrap();
        assert!(frames.is_empty());
    }

    /// Whether Whisper-tiny is reachable, pointing `DOCLING_ASR_*` at the
    /// workspace-root `models/asr/` when running from the crate dir (model
    /// resolution is CWD-relative).
    fn asr_models_ready() -> bool {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models/asr");
        if root.join("encoder_model.onnx").exists() {
            std::env::set_var("DOCLING_ASR_ENCODER", root.join("encoder_model.onnx"));
            std::env::set_var("DOCLING_ASR_DECODER", root.join("decoder_model.onnx"));
            std::env::set_var("DOCLING_ASR_VOCAB", root.join("vocab.json"));
            return true;
        }
        docling_asr::models_available()
    }

    #[test]
    fn video_document_interleaves_frames_with_transcript() {
        if !ffmpeg_available() || !asr_models_ready() {
            eprintln!("skipping: needs ffmpeg and the Whisper models");
            return;
        }
        let doc = convert_video(&fixture("sample_10s_video-mkv.mkv"), "s.mkv", None, 4).unwrap();
        let mut pictures = 0;
        let mut paragraphs = 0;
        for node in &doc.nodes {
            match node {
                Node::Picture { caption, image, .. } => {
                    pictures += 1;
                    assert!(caption.as_deref().unwrap_or("").starts_with("[time: "));
                    let img = image.as_ref().expect("frame is embedded");
                    assert_eq!(img.mimetype, "image/png");
                    assert_eq!((img.width, img.height), (64, 64));
                }
                Node::Paragraph { text } => {
                    paragraphs += 1;
                    assert!(text.starts_with("[time: "));
                }
                other => panic!("unexpected node {other:?}"),
            }
        }
        assert!(pictures >= 2, "expected sampled frames, got {pictures}");
        assert!(paragraphs >= 1, "expected transcript paragraphs");
        // The frame at t=0 precedes the first transcript segment.
        assert!(matches!(doc.nodes.first(), Some(Node::Picture { .. })));
    }

    /// A video with no audio track still converts — to its frames alone.
    #[test]
    fn video_without_audio_track_converts_to_frames_only() {
        if !ffmpeg_available() || !asr_models_ready() {
            eprintln!("skipping: needs ffmpeg and the Whisper models");
            return;
        }
        // Strip the audio track off a fixture on the fly (`-an -c copy`)
        // rather than committing another binary fixture.
        let dir = TempDir::new().unwrap();
        let src = dir.path.join("in.mkv");
        let out = dir.path.join("noaudio.mkv");
        std::fs::write(&src, fixture("sample_10s_video-mkv.mkv")).unwrap();
        let status = Command::new(ffmpeg_bin())
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(&src)
            .args(["-an", "-c", "copy", "-y"])
            .arg(&out)
            .status()
            .unwrap();
        assert!(status.success());
        let doc = convert_video(&std::fs::read(&out).unwrap(), "noaudio.mkv", None, 4).unwrap();
        assert!(!doc.nodes.is_empty());
        assert!(doc.nodes.iter().all(|n| matches!(n, Node::Picture { .. })));
    }
}
