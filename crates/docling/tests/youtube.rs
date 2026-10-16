//! A YouTube watch URL → its title, metadata and caption transcript (#681):
//! the converter routes an HTML source by the URL it was fetched from, the
//! backend fetches the caption track the page names, and the cues come
//! back as timed items. YouTube itself is stood in for by a local HTTP
//! server serving the recorded-shape fixtures (`tests/data/youtube`), the
//! page's caption `baseUrl`s rewritten to it.

#![cfg(feature = "youtube")]

use std::io::{Read as _, Write as _};
use std::path::Path;

use docling::{DocumentConverter, InputFormat, SourceDocument};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/youtube")
            .join(name),
    )
    .unwrap()
}

/// YouTube's `timedtext` endpoint on 127.0.0.1: answers `lang=en` /
/// `lang=de` with the matching captions (the auto-generated track when
/// `kind=asr`), a translation request (`tlang=`) with a marked copy, and
/// anything else with 404. Requests are recorded for the assertions.
struct Timedtext {
    url: String,
    requests: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Timedtext {
    fn start() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (log, flag) = (requests.clone(), stop.clone());
        std::thread::spawn(move || {
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                log.lock().unwrap().push(path.clone());
                let query = path.split_once('?').map_or("", |(_, q)| q);
                let param = |name: &str| {
                    query
                        .split('&')
                        .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
                        .map(str::to_string)
                };
                let body: Option<String> = if !path.starts_with("/api/timedtext")
                    || param("fmt").as_deref() != Some("vtt")
                {
                    None
                } else {
                    match (
                        param("lang").as_deref(),
                        param("kind").as_deref(),
                        param("tlang"),
                    ) {
                        (Some("en"), Some("asr"), None) => Some(fixture("captions_en_asr.vtt")),
                        (Some("en"), None, None) => Some(fixture("captions_en.vtt")),
                        (Some("de"), None, None) => Some(fixture("captions_de.vtt")),
                        (Some("en"), None, Some(t)) => Some(
                            fixture("captions_en.vtt")
                                .replace("docling.rs is", &format!("[{t}] docling.rs is")),
                        ),
                        _ => None,
                    }
                };
                let response = match body {
                    Some(vtt) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/vtt; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{vtt}",
                        vtt.len()
                    ),
                    None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            url,
            requests,
            stop,
        }
    }

    /// The watch page with its caption URLs pointing here.
    fn page(&self, name: &str) -> Vec<u8> {
        fixture(name)
            .replace(
                "https://www.youtube.com/api/timedtext",
                &format!("{}/api/timedtext", self.url),
            )
            .into_bytes()
    }
}

impl Drop for Timedtext {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

const WATCH_URL: &str = "https://www.youtube.com/watch?v=x7Zq9KfR2bE";

fn watch_source(server: &Timedtext, name: &str, url: &str) -> SourceDocument {
    // The CLI, serve and the Python package name a fetched page by its URL
    // path (`watch`, typed `text/html`) and record the URL as its base.
    SourceDocument::from_bytes("watch.html", InputFormat::Html, server.page(name))
        .with_base_url(url)
}

/// The caption fetch goes to 127.0.0.1 here: lift the SSRF block the way a
/// local deployment does.
fn allow_local() {
    std::env::set_var("DOCLING_RS_ALLOW_PRIVATE_IP_FETCH", "1");
}

/// #681's acceptance: a recorded watch page produces a timed transcript —
/// MarkItDown's `# YouTube` / `## title` / metadata / description shape,
/// the cues as timed items that `--to vtt` writes back, the transcript's
/// language from `asr_lang`.
#[test]
fn a_watch_page_converts_to_its_timed_transcript() {
    allow_local();
    let server = Timedtext::start();
    let result = DocumentConverter::new()
        .convert(watch_source(&server, "watch.html", WATCH_URL))
        .expect("converts");
    assert_eq!(result.format, InputFormat::Html);
    let md = result.document.export_to_markdown();
    assert!(
        md.starts_with(
            "# YouTube\n\n## Introducing docling.rs\n\n### Video Metadata\n\n- **Views:** 1234\n"
        ),
        "{md}"
    );
    assert!(
        md.contains("- **Runtime:** PT1M5S\n\n### Description\n\nA tour of the converter"),
        "{md}"
    );
    assert!(
        md.ends_with(
            "### Transcript\n\ndocling.rs is one binary that converts\n\n\
             PDF, Office and web documents to Markdown\n\n\
             and JSON — no Python, no models to install\n\n\
             every format goes through the same document model\n"
        ),
        "{md}"
    );
    assert!(
        !md.contains("ytInitialPlayerResponse") && !md.contains("<meta"),
        "{md}"
    );
    // Timed items: the VTT export has the cues with their timestamps.
    let vtt = result.document.export_to_vtt();
    assert!(
        vtt.contains("00:00:00.240 --> 00:00:03.120\ndocling.rs is one binary that converts\n"),
        "{vtt}"
    );
    assert!(
        vtt.contains(
            "00:00:30.000 --> 00:00:33.500\nevery format goes through the same document model"
        ),
        "{vtt}"
    );
    let json = result.document.export_to_json();
    assert!(
        json.contains("\"start_time\": 30.0") && json.contains("\"end_time\": 33.5"),
        "{json}"
    );
    // The written English track was fetched as WebVTT, nothing else.
    let requests = server.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(
        requests[0].starts_with("/api/timedtext?v=x7Zq9KfR2bE&")
            && requests[0].ends_with("&lang=en&fmt=vtt"),
        "{requests:?}"
    );
    assert_eq!(
        result.document.metadata.title.as_deref(),
        Some("Introducing docling.rs")
    );

    // `asr_lang`: the German track; a language YouTube only translates
    // into goes through `tlang`; one it has nothing for is an error that
    // lists the tracks.
    let german = DocumentConverter::new()
        .asr_lang(Some("de".into()))
        .convert(watch_source(&server, "watch.html", WATCH_URL))
        .unwrap();
    assert!(
        german
            .document
            .export_to_markdown()
            .contains("### Transcript\n\ndocling.rs ist eine Binärdatei, die\n"),
        "{}",
        german.document.export_to_markdown()
    );
    let french = DocumentConverter::new()
        .asr_lang(Some("fr".into()))
        .convert(watch_source(&server, "watch.html", WATCH_URL))
        .unwrap();
    assert!(
        french
            .document
            .export_to_markdown()
            .contains("[fr] docling.rs is one binary"),
        "{}",
        french.document.export_to_markdown()
    );
    let err = DocumentConverter::new()
        .asr_lang(Some("ja".into()))
        .convert(watch_source(&server, "watch.html", WATCH_URL))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("has no ja captions (available: en, de)"),
        "{err}"
    );
}

/// A video with no captions fails with a clear error naming it; a caption
/// endpoint that answers 404 fails as a fetch error, not as an empty
/// transcript; a YouTube page saved to disk (no URL) is ordinary HTML.
#[test]
fn no_captions_is_a_clear_error_and_a_saved_page_is_html() {
    allow_local();
    let server = Timedtext::start();
    let err = DocumentConverter::new()
        .convert(watch_source(
            &server,
            "watch_no_captions.html",
            "https://www.youtube.com/watch?v=q2WkY8rPz3M",
        ))
        .unwrap_err()
        .to_string();
    assert!(err.contains("video q2WkY8rPz3M has no captions"), "{err}");
    assert!(server.requests.lock().unwrap().is_empty());

    let moved = String::from_utf8(server.page("watch.html"))
        .unwrap()
        .replace("/api/timedtext", "/gone");
    let gone = SourceDocument::from_bytes("watch.html", InputFormat::Html, moved.into_bytes())
        .with_base_url("https://youtu.be/x7Zq9KfR2bE");
    let err = DocumentConverter::new()
        .convert(gone)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("fetching the en transcript of video x7Zq9KfR2bE") && err.contains("404"),
        "{err}"
    );

    // No URL: the page converts as the HTML it is — nothing is fetched.
    let fetched_so_far = server.requests.lock().unwrap().len();
    let saved =
        SourceDocument::from_bytes("watch.html", InputFormat::Html, server.page("watch.html"));
    let doc = DocumentConverter::new().convert(saved).unwrap().document;
    let md = doc.export_to_markdown();
    assert!(!md.contains("### Transcript"), "{md}");
    assert_eq!(server.requests.lock().unwrap().len(), fetched_so_far);
    // Another site's page with a `watch?v=` path is not YouTube either.
    let other =
        SourceDocument::from_bytes("watch.html", InputFormat::Html, server.page("watch.html"))
            .with_base_url("https://example.com/watch?v=x7Zq9KfR2bE");
    let md = DocumentConverter::new()
        .convert(other)
        .unwrap()
        .document
        .export_to_markdown();
    assert!(!md.contains("### Transcript"), "{md}");
    assert_eq!(server.requests.lock().unwrap().len(), fetched_so_far);
}
