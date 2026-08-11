//! `docling-rs serve` — a long-running HTTP server over the docling.rs
//! converter, the analogue of Python's `docling-serve`.
//!
//! Endpoints:
//!
//! | Method | Path          | Description                                        |
//! |--------|---------------|----------------------------------------------------|
//! | GET    | `/`           | API docs + an interactive test form                |
//! | POST   | `/v1/convert` | convert an upload (multipart) or a URL (JSON body) |
//! | POST   | `/v1/convert/async` | same request, returns a task id (#182)       |
//! | GET    | `/v1/status/{id}` | async job status (pending/started/success/failure) |
//! | GET    | `/v1/result/{id}` | async job result (the sync response, stored)   |
//! | GET    | `/v1/config`  | server capabilities (`{"allow_url_fetch": bool}`)  |
//! | GET    | `/health`     | liveness probe                                     |
//! | GET    | `/ready`      | readiness probe (200 once models are warm)         |
//! | GET    | `/openapi.yaml` | OpenAPI 3.1 description of the API               |
//! | GET    | `/logo.svg`   | the playground's logo                              |
//!
//! `POST /v1/convert` accepts either `multipart/form-data` with a `file` part
//! (the filename's extension selects the input format; **several file parts
//! make a batch**, #182 — the response is then a JSON `results` array with
//! per-item status) or an `application/json` body
//! `{"url": "https://…", "file_name"?: "override.pdf"}`.
//! Options ride along as multipart text parts, JSON fields, or query
//! parameters (body wins over query):
//!
//! - `to` — `md` (default) | `json` | `dclx` | `chunks`
//! - `strict` — cleaner Markdown instead of docling-legacy output
//! - `images` — `placeholder` (default) | `embedded` (Markdown only)
//! - `no_ocr`, `no_table_former`, `force_full_page_ocr`, `no_text_panels` — PDF/image pipeline switches
//! - `pages` — PDF page window `A-B` / `N` (1-based inclusive, #80)
//! - `ocr_lang` — OCR recognition language for scanned pages: `en` (default)
//!   | `ch` (the multilingual docling-conformance model)
//! - `fetch_images` — resolve external `<img src>` for HTML/EPUB (outbound
//!   fetch, so honored only under `--allow-url-fetch`)
//!
//! Markdown converts through the streaming serializer and the response body
//! streams page by page (chunked transfer); `json`/`dclx`/`chunks` (and every
//! batch/async result) buffer.
//!
//! Responses carry the conversion-confidence report (#183) when the PDF/image
//! ML pipeline ran: an `X-Docling-Confidence` header with the document-level
//! summary (grades + scores, docling's `ConfidenceReport` semantics) on every
//! output format, and a top-level `"confidence"` key with the per-page
//! breakdown appended to `to=json` bodies. Declarative conversions have no ML
//! stages and carry neither.
//!
//! `POST /v1/convert/async` (#182) accepts exactly the `/v1/convert` request
//! and answers `202 {"task_id": …}` immediately; the job queues on the same
//! concurrency semaphore (reusing the warm pipeline) and the result is
//! fetched with `GET /v1/result/{id}` once `GET /v1/status/{id}` reports
//! `success`. Results are held for `--result-ttl` seconds; at most
//! `--queue-size` jobs may be queued/unfetched at once (429 beyond that).
//!
//! One warm [`Pipeline`] (layout/OCR/TableFormer sessions) is shared across
//! requests behind a mutex — PDF/image conversions serialize on it instead of
//! reloading models. Declarative formats convert on blocking threads and run
//! concurrently. A semaphore bounds total in-flight conversions
//! (`--concurrency`); excess requests queue.
//!
//! Security: URL fetching makes the server issue outbound requests (SSRF
//! surface), so it is **off by default** — enable with `--allow-url-fetch`.
//! Even when enabled, targets that resolve to a private/loopback/link-local
//! address are refused and redirects are disabled. The server itself has no
//! authentication: bind to loopback (the default) or front with a policy/auth
//! proxy before exposing it.

use std::io::Read;
use std::net::ToSocketAddrs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, FromRequest, Multipart, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use docling::{
    DoclingDocument, DocumentConverter, ImageMode, InputFormat, Pipeline, SourceDocument,
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Semaphore;

/// Server configuration (see the binary's `--help` for the flag spellings).
#[derive(Clone, Debug)]
pub struct ServeConfig {
    /// Bind address, e.g. `127.0.0.1:5001`.
    pub addr: String,
    /// Maximum conversions in flight; further requests queue on the semaphore.
    pub concurrency: usize,
    /// Maximum accepted request body (multipart upload) in bytes.
    pub max_body_bytes: usize,
    /// Load the PDF/image models at startup so `/ready` flips only when the
    /// first conversion would be fast. Off: models load lazily on first use.
    pub warmup: bool,
    /// Allow `{"url": …}` inputs (outbound fetch — SSRF surface). Off by
    /// default: even with the built-in private/loopback/link-local IP guard,
    /// letting a caller name the fetch target is a deliberate exposure that a
    /// deployment must opt into (`--allow-url-fetch`).
    pub allow_url_fetch: bool,
    /// Default `strict` for requests that don't set it.
    pub strict: bool,
    /// Maximum async jobs (#182) waiting or running at once; further
    /// `POST /v1/convert/async` submissions are refused with 429. Bounds the
    /// memory held by queued request bytes.
    pub queue_size: usize,
    /// How long a finished async job's result stays fetchable before it is
    /// evicted (idle results are the other thing holding memory).
    pub result_ttl_secs: u64,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            addr: "127.0.0.1:5001".into(),
            concurrency: 2,
            max_body_bytes: 256 * 1024 * 1024,
            warmup: false,
            allow_url_fetch: false,
            strict: false,
            queue_size: 16,
            result_ttl_secs: 600,
        }
    }
}

struct AppState {
    /// Warm ML pipeline (mutable ONNX sessions) — one PDF/image conversion at
    /// a time, but the models stay loaded across requests.
    pipeline: Mutex<Option<Pipeline>>,
    /// Bounds total in-flight conversions (`Arc` so a permit can move into
    /// a streaming response's worker and outlive the handler).
    permits: Arc<Semaphore>,
    /// Async conversion jobs (#182), keyed by task id.
    jobs: Mutex<std::collections::HashMap<String, Job>>,
    ready: AtomicBool,
    cfg: ServeConfig,
}

/// Build the router (exposed separately from [`serve`] for tests).
pub fn router(cfg: ServeConfig) -> Router {
    let state = Arc::new(AppState {
        pipeline: Mutex::new(None),
        permits: Arc::new(Semaphore::new(cfg.concurrency.max(1))),
        jobs: Mutex::new(std::collections::HashMap::new()),
        ready: AtomicBool::new(!cfg.warmup),
        cfg: cfg.clone(),
    });
    if cfg.warmup {
        let st = state.clone();
        // Blocking model load off the runtime; readiness flips when done.
        tokio::task::spawn_blocking(move || {
            match Pipeline::new() {
                Ok(p) => *st.pipeline.lock().unwrap() = Some(p),
                Err(e) => eprintln!("warmup: pipeline load failed: {e}"),
            }
            st.ready.store(true, Ordering::Release);
        });
    }
    Router::new()
        // Docs + test form, like the original docling-serve's playground.
        .route(
            "/",
            get(|| async { axum::response::Html(include_str!("index.html")) }),
        )
        // The page's logo and the machine-readable API description. Both are
        // baked into the binary, so a server needs no static-file directory.
        .route(
            "/logo.svg",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "image/svg+xml")],
                    include_str!("logo.svg"),
                )
            }),
        )
        .route(
            "/openapi.yaml",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/yaml")],
                    include_str!("openapi.yaml"),
                )
            }),
        )
        .route("/health", get(|| async { Json(json!({"status": "ok"})) }))
        .route("/ready", get(ready))
        .route("/v1/config", get(config))
        .route("/v1/convert", post(convert))
        .route("/v1/convert/async", post(convert_async))
        .route("/v1/status/{id}", get(job_status))
        .route("/v1/result/{id}", get(job_result))
        .layer(DefaultBodyLimit::max(cfg.max_body_bytes))
        .with_state(state)
}

/// Bind and serve until SIGINT/SIGTERM; in-flight requests finish (graceful
/// shutdown).
pub async fn serve(cfg: ServeConfig) -> Result<(), String> {
    let addr = cfg.addr.clone();
    let app = router(cfg);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("cannot bind {addr}: {e}"))?;
    eprintln!("docling-serve listening on http://{addr}");
    // Log the resolved model set once at startup: when two deployments
    // convert differently, this is the first thing to compare (also served
    // live at /v1/config).
    for m in docling::model_inventory() {
        if m.found {
            eprintln!(
                "docling-serve: model {:<20} {} ({:.1} MB)",
                m.stage,
                m.path,
                m.bytes as f64 / 1_048_576.0
            );
        } else {
            eprintln!("docling-serve: model {:<20} {} (MISSING)", m.stage, m.path);
        }
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| format!("server error: {e}"))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
    eprintln!("docling-serve: shutdown signal received, draining in-flight requests");
}

/// Capabilities the built-in UI adapts to. Currently just whether `{"url": …}`
/// inputs are accepted (`--allow-url-fetch`) — the UI greys out the URL option
/// and explains why when this is false, instead of letting the user hit a 422.
async fn config(State(state): State<Arc<AppState>>) -> Response {
    Json(json!({
        "allow_url_fetch": state.cfg.allow_url_fetch,
        // Which model file each pipeline stage would load right now (resolved
        // per request — CWD-relative with env overrides, so this is the truth,
        // not a startup snapshot). "Two servers convert the same PDF
        // differently" is almost always this list differing; the size column
        // distinguishes an int8 quant from an fp32 graph at a glance.
        "models": docling::model_inventory().into_iter().map(|m| json!({
            "stage": m.stage,
            "path": m.path,
            "found": m.found,
            "bytes": m.bytes,
        })).collect::<Vec<_>>(),
    }))
    .into_response()
}

async fn ready(State(state): State<Arc<AppState>>) -> Response {
    if state.ready.load(Ordering::Acquire) {
        Json(json!({"status": "ready"})).into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "warming_up"})),
        )
            .into_response()
    }
}

/// Request options, merged from query parameters and body fields.
#[derive(Clone, Debug, Default, Deserialize)]
struct ConvertOptions {
    to: Option<String>,
    strict: Option<bool>,
    images: Option<String>,
    no_ocr: Option<bool>,
    force_full_page_ocr: Option<bool>,
    no_table_former: Option<bool>,
    /// Keep text-panel pictures as pictures (#173): disable the #157 demotion
    /// of uncaptioned dense-text "picture" regions into paragraphs.
    no_text_panels: Option<bool>,
    fetch_images: Option<bool>,
    asr_model: Option<String>,
    /// ASR transcription language for audio/video input: a Whisper code
    /// (`en`, `de`, …) or `auto` (default) — detected from the first
    /// 30 seconds. Unknown codes fail the conversion with a clear error.
    asr_lang: Option<String>,
    /// Max frames sampled from a video input (0 = transcript only; needs the
    /// server to have the ffmpeg binary).
    video_frames: Option<usize>,
    /// PDF page window, `"A-B"` or a single `"N"` (1-based inclusive — #80).
    pages: Option<String>,
    /// OCR recognition language for scanned pages: `en` (default) | `ch`.
    ocr_lang: Option<String>,
}

impl ConvertOptions {
    fn merge_over(self, base: ConvertOptions) -> ConvertOptions {
        ConvertOptions {
            to: self.to.or(base.to),
            strict: self.strict.or(base.strict),
            images: self.images.or(base.images),
            no_ocr: self.no_ocr.or(base.no_ocr),
            force_full_page_ocr: self.force_full_page_ocr.or(base.force_full_page_ocr),
            no_table_former: self.no_table_former.or(base.no_table_former),
            no_text_panels: self.no_text_panels.or(base.no_text_panels),
            fetch_images: self.fetch_images.or(base.fetch_images),
            asr_model: self.asr_model.or(base.asr_model),
            asr_lang: self.asr_lang.or(base.asr_lang),
            video_frames: self.video_frames.or(base.video_frames),
            pages: self.pages.or(base.pages),
            ocr_lang: self.ocr_lang.or(base.ocr_lang),
        }
    }
}

/// JSON body for URL inputs.
#[derive(Debug, Deserialize)]
struct UrlRequest {
    url: String,
    /// Overrides the name (and thus format-selecting extension) taken from
    /// the URL path's last segment.
    file_name: Option<String>,
    #[serde(flatten)]
    options: ConvertOptions,
}

enum ApiError {
    Bad(String),
    Unsupported(String),
    Internal(String),
    /// The async job queue is full (#182) — retry later.
    Busy(String),
}

/// The HTTP status + message an [`ApiError`] answers with (also stored on a
/// failed async job so `/v1/result/{id}` reproduces the sync status).
fn api_error_parts(e: ApiError) -> (StatusCode, String) {
    match e {
        ApiError::Bad(m) => (StatusCode::BAD_REQUEST, m),
        ApiError::Unsupported(m) => (StatusCode::UNPROCESSABLE_ENTITY, m),
        ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        ApiError::Busy(m) => (StatusCode::TOO_MANY_REQUESTS, m),
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = api_error_parts(self);
        (status, Json(json!({"error": msg}))).into_response()
    }
}

/// One parsed upload: the source, or the filename with why it couldn't become
/// one (a batch converts around a bad item; a single-file request propagates
/// the error as its response).
type SourceItem = Result<SourceDocument, (String, ApiError)>;

/// Parse a conversion request body — `multipart/form-data` uploads (one or
/// more `file` parts, #182 batch) or an `application/json` `{"url": …}` — into
/// sources plus the merged options. Shared by the sync and async endpoints so
/// both accept exactly the same requests (and reject bad ones synchronously).
async fn parse_convert_request(
    state: &Arc<AppState>,
    query: ConvertOptions,
    headers: HeaderMap,
    body: axum::extract::Request,
) -> Result<(Vec<SourceItem>, ConvertOptions), ApiError> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if content_type.starts_with("multipart/form-data") {
        let multipart = Multipart::from_request(body, &())
            .await
            .map_err(|e| ApiError::Bad(format!("bad multipart body: {e}")))?;
        read_multipart(multipart, query).await
    } else if content_type.starts_with("application/json") {
        let bytes = axum::body::to_bytes(body.into_body(), state.cfg.max_body_bytes)
            .await
            .map_err(|e| ApiError::Bad(format!("bad body: {e}")))?;
        let req: UrlRequest = serde_json::from_slice(&bytes)
            .map_err(|e| ApiError::Bad(format!("bad JSON body: {e}")))?;
        if !state.cfg.allow_url_fetch {
            return Err(ApiError::Unsupported(
                "URL inputs are disabled; start docling-serve with --allow-url-fetch \
                 (SSRF surface — see docs/SECURITY.md), or upload the file instead"
                    .into(),
            ));
        }
        let options = req.options.clone().merge_over(query);
        let url = req.url.clone();
        let name = req.file_name.clone();
        let source = tokio::task::spawn_blocking(move || fetch_url(&url, name.as_deref()))
            .await
            .map_err(|e| ApiError::Internal(format!("fetch task: {e}")))??;
        Ok((vec![Ok(source)], options))
    } else {
        Err(ApiError::Bad(
            "expected multipart/form-data (file upload) or application/json ({\"url\": …})".into(),
        ))
    }
}

/// Validate the `to` / `images` options (shared by sync and async so an async
/// submission fails fast instead of parking a doomed job in the queue).
fn validate_output(options: &ConvertOptions) -> Result<(String, ImageMode), ApiError> {
    let to = options.to.clone().unwrap_or_else(|| "md".into());
    if !matches!(to.as_str(), "md" | "markdown" | "json" | "dclx" | "chunks") {
        return Err(ApiError::Bad(format!(
            "unknown to='{to}' (expected: md, json, dclx, chunks)"
        )));
    }
    let image_mode = match options.images.as_deref().unwrap_or("placeholder") {
        "placeholder" => ImageMode::Placeholder,
        "embedded" => ImageMode::Embedded,
        other => {
            return Err(ApiError::Bad(format!(
                "unknown images='{other}' (expected: placeholder, embedded)"
            )))
        }
    };
    Ok((to, image_mode))
}

async fn convert(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ConvertOptions>,
    headers: HeaderMap,
    body: axum::extract::Request,
) -> Result<Response, ApiError> {
    let (sources, options) = parse_convert_request(&state, query, headers, body).await?;
    let (to, image_mode) = validate_output(&options)?;

    // Bound total in-flight conversions; excess requests queue here. The
    // permit is owned so the streaming path can hold it until the response
    // body finishes, not just until the handler returns.
    let permit = state
        .permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| ApiError::Internal(format!("semaphore: {e}")))?;

    // A single Markdown conversion streams; everything else (other formats,
    // #182 batches — which need per-item framing) buffers.
    let is_markdown = matches!(to.as_str(), "md" | "markdown");
    if is_markdown && sources.len() == 1 {
        let source = sources
            .into_iter()
            .next()
            .expect("checked len")
            .map_err(|(_, e)| e)?;
        return stream_markdown(state.clone(), source, options, image_mode, permit).await;
    }

    let st = state.clone();
    let stored = tokio::task::spawn_blocking(move || {
        run_conversion(&st, sources, &options, &to, image_mode)
    })
    .await
    .map_err(|e| ApiError::Internal(format!("convert task: {e}")))?;
    drop(permit);
    Ok(stored?.into_response())
}

/// One async conversion job (#182).
struct Job {
    state: JobState,
    /// When the job left the queue (finished or failed) — drives TTL eviction.
    done_at: Option<std::time::Instant>,
}

enum JobState {
    /// Waiting for a conversion slot (the shared semaphore).
    Pending,
    Started,
    Success(StoredResponse),
    /// The HTTP status the sync endpoint would have answered with, plus the
    /// error message.
    Failure(StatusCode, String),
}

impl JobState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Started => "started",
            Self::Success(_) => "success",
            Self::Failure(..) => "failure",
        }
    }
}

/// Evict finished jobs whose result has outlived the TTL. Called from the job
/// endpoints — no background sweeper thread needed, since memory only ever
/// accumulates through those same endpoints' submissions.
fn purge_expired(jobs: &mut std::collections::HashMap<String, Job>, ttl_secs: u64) {
    let ttl = std::time::Duration::from_secs(ttl_secs);
    jobs.retain(|_, job| job.done_at.is_none_or(|done| done.elapsed() < ttl));
}

/// An unguessable task id. The result endpoint is unauthenticated (like the
/// rest of the API), so the id doubles as the fetch capability: 128 bits from
/// two independently random-seeded `RandomState` hashers — not a substitute
/// for real authentication (front the server with one), but not enumerable
/// either.
fn task_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h1 = std::collections::hash_map::RandomState::new().build_hasher();
    let mut h2 = std::collections::hash_map::RandomState::new().build_hasher();
    h1.write_u64(0);
    h2.write_u64(1);
    format!("{:016x}{:016x}", h1.finish(), h2.finish())
}

/// `POST /v1/convert/async` (#182): accept the same request as `/v1/convert`,
/// but return a task id immediately instead of holding the connection for the
/// duration of the conversion. The job queues on the same semaphore as sync
/// requests (reusing the warm pipeline); poll `GET /v1/status/{id}` and fetch
/// `GET /v1/result/{id}`.
async fn convert_async(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ConvertOptions>,
    headers: HeaderMap,
    body: axum::extract::Request,
) -> Result<Response, ApiError> {
    let (mut sources, options) = parse_convert_request(&state, query, headers, body).await?;
    let (to, image_mode) = validate_output(&options)?;
    // A single unconvertible upload fails the submission itself (a batch
    // converts around bad items) — same surface as the sync endpoint, and no
    // doomed job occupies the queue.
    if sources.len() == 1 && sources[0].is_err() {
        let (_, e) = sources.remove(0).expect_err("checked is_err");
        return Err(e);
    }

    let id = task_id();
    {
        let mut jobs = state.jobs.lock().unwrap();
        purge_expired(&mut jobs, state.cfg.result_ttl_secs);
        // The bound counts jobs still holding request/result memory — queued,
        // running, or finished-but-unfetched — so a burst can't grow the map
        // (and the upload bytes it holds) without limit.
        if jobs.len() >= state.cfg.queue_size {
            return Err(ApiError::Busy(format!(
                "job queue is full ({} jobs); retry after fetching or expiring results",
                jobs.len()
            )));
        }
        jobs.insert(
            id.clone(),
            Job {
                state: JobState::Pending,
                done_at: None,
            },
        );
    }

    let st = state.clone();
    let job_id = id.clone();
    tokio::spawn(async move {
        // Queue on the shared conversion semaphore. Closed-semaphore errors
        // only happen at shutdown; the job then just stays pending until the
        // process exits.
        let Ok(permit) = st.permits.clone().acquire_owned().await else {
            return;
        };
        if let Some(job) = st.jobs.lock().unwrap().get_mut(&job_id) {
            job.state = JobState::Started;
        } else {
            return; // evicted while queued (TTL abuse would need days)
        }
        let stx = st.clone();
        let opts = options;
        let outcome = tokio::task::spawn_blocking(move || {
            run_conversion(&stx, sources, &opts, &to, image_mode)
        })
        .await
        .map_err(|e| ApiError::Internal(format!("convert task: {e}")))
        .and_then(|r| r);
        drop(permit);
        if let Some(job) = st.jobs.lock().unwrap().get_mut(&job_id) {
            job.state = match outcome {
                Ok(stored) => JobState::Success(stored),
                Err(e) => {
                    let (status, msg) = api_error_parts(e);
                    JobState::Failure(status, msg)
                }
            };
            job.done_at = Some(std::time::Instant::now());
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "task_id": id, "task_status": "pending" })),
    )
        .into_response())
}

/// `GET /v1/status/{id}` (#182).
async fn job_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let mut jobs = state.jobs.lock().unwrap();
    purge_expired(&mut jobs, state.cfg.result_ttl_secs);
    match jobs.get(&id) {
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "unknown task id (never submitted, or its result expired)"})),
        )
            .into_response(),
        Some(job) => {
            let mut body = json!({ "task_id": id, "task_status": job.state.as_str() });
            if let JobState::Failure(_, msg) = &job.state {
                body["error"] = json!(msg);
            }
            Json(body).into_response()
        }
    }
}

/// `GET /v1/result/{id}` (#182): the conversion output with the same content
/// type / headers the sync endpoint would have used. Not ready yet → 202 with
/// the status body; failed → the sync endpoint's error status; unknown or
/// expired → 404. The result stays fetchable until the TTL evicts it.
async fn job_result(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let mut jobs = state.jobs.lock().unwrap();
    purge_expired(&mut jobs, state.cfg.result_ttl_secs);
    match jobs.get(&id) {
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "unknown task id (never submitted, or its result expired)"})),
        )
            .into_response(),
        Some(job) => match &job.state {
            JobState::Pending | JobState::Started => (
                StatusCode::ACCEPTED,
                Json(json!({ "task_id": id, "task_status": job.state.as_str() })),
            )
                .into_response(),
            JobState::Failure(status, msg) => {
                (*status, Json(json!({"error": msg}))).into_response()
            }
            // Clone rather than remove: the result stays re-fetchable until
            // the TTL evicts it (a client retrying a dropped response must not
            // find a 404).
            JobState::Success(stored) => StoredResponse {
                content_type: stored.content_type,
                disposition: stored.disposition.clone(),
                confidence: stored.confidence.clone(),
                body: stored.body.clone(),
            }
            .into_response(),
        },
    }
}

/// A fully materialized conversion response — what a buffered sync request
/// answers with, and what an async job (#182) stores until the client fetches
/// `/v1/result/{id}`.
struct StoredResponse {
    content_type: &'static str,
    /// `Content-Disposition` for downloads (dclx).
    disposition: Option<String>,
    /// The `X-Docling-Confidence` summary (#183), when the pipeline made one.
    confidence: Option<header::HeaderValue>,
    body: Vec<u8>,
}

impl StoredResponse {
    fn into_response(self) -> Response {
        let mut response = ([(header::CONTENT_TYPE, self.content_type)], self.body).into_response();
        if let Some(d) = self.disposition {
            if let Ok(v) = header::HeaderValue::from_str(&d) {
                response
                    .headers_mut()
                    .insert(header::CONTENT_DISPOSITION, v);
            }
        }
        if let Some(v) = self.confidence {
            response.headers_mut().insert("x-docling-confidence", v);
        }
        response
    }
}

/// Convert one or more sources on the current (blocking) thread and serialize
/// the result. A single source renders as the plain output format; multiple
/// sources (#182 batch) render as a JSON results array with per-item status,
/// so one bad file fails its item, not the whole batch.
fn run_conversion(
    state: &AppState,
    sources: Vec<SourceItem>,
    options: &ConvertOptions,
    to: &str,
    image_mode: ImageMode,
) -> Result<StoredResponse, ApiError> {
    if sources.len() == 1 {
        let source = sources
            .into_iter()
            .next()
            .expect("checked len")
            .map_err(|(_, e)| e)?;
        let name = source.name.clone();
        let document = convert_document(state, source, options)?;
        return Ok(render_stored(
            state, to, image_mode, &name, &document, options,
        ));
    }
    let items: Vec<serde_json::Value> = sources
        .into_iter()
        .map(|item| {
            let source = match item {
                Ok(source) => source,
                Err((name, e)) => {
                    return json!({
                        "name": name,
                        "status": "failure",
                        "error": api_error_message(e),
                    })
                }
            };
            let name = source.name.clone();
            match convert_document(state, source, options) {
                Ok(document) => batch_item(state, to, image_mode, &name, &document, options),
                Err(e) => json!({
                    "name": name,
                    "status": "failure",
                    "error": api_error_message(e),
                }),
            }
        })
        .collect();
    Ok(StoredResponse {
        content_type: "application/json",
        disposition: None,
        confidence: None,
        body: serde_json::to_vec_pretty(&json!({ "results": items }))
            .expect("batch JSON serializes"),
    })
}

/// Serialize a converted document for a buffered (non-streaming) response.
/// Responses carry the conversion-confidence report (#183) when the ML
/// pipeline produced one: as an `X-Docling-Confidence` summary header
/// everywhere, and as a top-level `"confidence"` key (with the per-page
/// breakdown) appended to the `json` body — the document keys themselves are
/// untouched, so the body still parses as a docling-JSON document.
fn render_stored(
    state: &AppState,
    to: &str,
    image_mode: ImageMode,
    name: &str,
    document: &DoclingDocument,
    options: &ConvertOptions,
) -> StoredResponse {
    let confidence = confidence_header(document);
    match to {
        "md" | "markdown" => StoredResponse {
            content_type: "text/markdown; charset=utf-8",
            disposition: None,
            confidence,
            body: markdown_string(state, document, image_mode, options).into_bytes(),
        },
        "json" => {
            let mut value = document.export_to_json_value();
            if let Some(report) = &document.confidence {
                value["confidence"] = report.to_json();
            }
            StoredResponse {
                content_type: "application/json",
                disposition: None,
                confidence,
                body: serde_json::to_vec_pretty(&value).expect("document JSON serializes"),
            }
        }
        "chunks" => {
            let mut warnings: Vec<String> = Vec::new();
            let mut records = docling::chunks::chunk_records(document, &mut |m| warnings.push(m));
            if !warnings.is_empty() {
                records["warnings"] = json!(warnings);
            }
            StoredResponse {
                content_type: "application/json",
                disposition: None,
                confidence,
                body: serde_json::to_vec(&records).expect("chunk records serialize"),
            }
        }
        "dclx" => StoredResponse {
            content_type: "application/octet-stream",
            disposition: Some(format!("attachment; filename=\"{name}.dclx\"")),
            confidence,
            body: docling::dclx::to_dclx_bytes(document),
        },
        _ => unreachable!("validated above"),
    }
}

/// One batch item (#182) as JSON. Text outputs inline as strings, the
/// docling-JSON document as an object, binary dclx as base64; the confidence
/// summary (#183) rides along as a sibling key where it isn't already inside
/// the document JSON.
fn batch_item(
    state: &AppState,
    to: &str,
    image_mode: ImageMode,
    name: &str,
    document: &DoclingDocument,
    options: &ConvertOptions,
) -> serde_json::Value {
    let mut item = json!({ "name": name, "status": "success" });
    match to {
        "md" | "markdown" => {
            item["md"] = json!(markdown_string(state, document, image_mode, options));
        }
        "json" => {
            let mut value = document.export_to_json_value();
            if let Some(report) = &document.confidence {
                value["confidence"] = report.to_json();
            }
            item["document"] = value;
        }
        "chunks" => {
            let mut warnings: Vec<String> = Vec::new();
            let mut records = docling::chunks::chunk_records(document, &mut |m| warnings.push(m));
            if !warnings.is_empty() {
                records["warnings"] = json!(warnings);
            }
            item["chunks"] = records;
        }
        "dclx" => {
            item["dclx_base64"] = json!(docling::base64::encode(&docling::dclx::to_dclx_bytes(
                document
            )));
        }
        _ => unreachable!("validated above"),
    }
    if to != "json" {
        if let Some(report) = &document.confidence {
            item["confidence"] = report.summary_json();
        }
    }
    item
}

/// Buffered Markdown export honoring the request's `strict` / `images`
/// options — the non-streaming counterpart of `stream_markdown`'s serializer
/// calls (batch items and async results can't stream).
fn markdown_string(
    state: &AppState,
    document: &DoclingDocument,
    image_mode: ImageMode,
    options: &ConvertOptions,
) -> String {
    let mut doc = document.clone();
    doc.strict_markdown = options.strict.unwrap_or(state.cfg.strict);
    match image_mode {
        ImageMode::Placeholder => doc.export_to_markdown(),
        _ => {
            doc.export_to_markdown_with_images(image_mode, "artifacts")
                .0
        }
    }
}

/// The document-level confidence summary as a header value (compact JSON, no
/// per-page breakdown — headers should stay small). `None` when the
/// conversion had no ML stages (declarative formats).
fn confidence_header(document: &DoclingDocument) -> Option<header::HeaderValue> {
    let report = document.confidence.as_ref()?;
    header::HeaderValue::from_str(&report.summary_json().to_string()).ok()
}

/// Read the multipart request: one or more `file` parts (bytes + filename —
/// several files make a #182 batch; `files` is accepted as an alias) plus
/// optional text parts mirroring the query options.
async fn read_multipart(
    mut multipart: Multipart,
    query: ConvertOptions,
) -> Result<(Vec<SourceItem>, ConvertOptions), ApiError> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut body_opts = ConvertOptions::default();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::Bad(format!("bad multipart field: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" | "files" => {
                let file_name = field
                    .file_name()
                    .map(|s| s.to_string())
                    .ok_or_else(|| ApiError::Bad("file part needs a filename".into()))?;
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::Bad(format!("reading upload: {e}")))?;
                files.push((file_name, bytes.to_vec()));
            }
            "to" | "images" => {
                let v = text_field(field).await?;
                match name.as_str() {
                    "to" => body_opts.to = Some(v),
                    _ => body_opts.images = Some(v),
                }
            }
            "asr_model" => body_opts.asr_model = Some(text_field(field).await?),
            "asr_lang" => body_opts.asr_lang = Some(text_field(field).await?),
            "pages" => body_opts.pages = Some(text_field(field).await?),
            "ocr_lang" => body_opts.ocr_lang = Some(text_field(field).await?),
            "video_frames" => {
                let v = text_field(field).await?;
                body_opts.video_frames = Some(v.parse().map_err(|_| {
                    ApiError::Bad(format!(
                        "video_frames must be a non-negative integer, got {v:?}"
                    ))
                })?);
            }
            "strict"
            | "no_ocr"
            | "no_table_former"
            | "force_full_page_ocr"
            | "no_text_panels"
            | "fetch_images" => {
                let v = text_field(field).await?;
                let b = matches!(v.as_str(), "1" | "true" | "yes" | "on");
                match name.as_str() {
                    "strict" => body_opts.strict = Some(b),
                    "no_ocr" => body_opts.no_ocr = Some(b),
                    "force_full_page_ocr" => body_opts.force_full_page_ocr = Some(b),
                    "no_table_former" => body_opts.no_table_former = Some(b),
                    "no_text_panels" => body_opts.no_text_panels = Some(b),
                    _ => body_opts.fetch_images = Some(b),
                }
            }
            _ => {} // unknown parts are ignored
        }
    }
    if files.is_empty() {
        return Err(ApiError::Bad("missing 'file' part".into()));
    }
    // Per-file errors (unknown extension) are deferred: a single-file request
    // propagates them as its response, a batch fails only that item.
    let sources = files
        .into_iter()
        .map(|(file_name, bytes)| {
            source_from_named_bytes(&file_name, bytes).map_err(|e| (file_name, e))
        })
        .collect();
    Ok((sources, body_opts.merge_over(query)))
}

async fn text_field(field: axum::extract::multipart::Field<'_>) -> Result<String, ApiError> {
    field
        .text()
        .await
        .map_err(|e| ApiError::Bad(format!("reading field: {e}")))
}

/// Build a [`SourceDocument`] from a filename (extension → format) and bytes.
fn source_from_named_bytes(file_name: &str, bytes: Vec<u8>) -> Result<SourceDocument, ApiError> {
    source_from_named_bytes_ct(file_name, bytes, None)
}

/// As [`source_from_named_bytes`], with an optional response `Content-Type` used
/// as a fallback when the name carries no usable extension — a URL like
/// `…/help/example-domains` has no `.html`, but the server reports
/// `text/html`, so it still converts.
fn source_from_named_bytes_ct(
    file_name: &str,
    bytes: Vec<u8>,
    content_type: Option<&str>,
) -> Result<SourceDocument, ApiError> {
    let ext = std::path::Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str());
    let format = ext
        .and_then(InputFormat::from_extension)
        .or_else(|| content_type.and_then(format_from_content_type))
        .ok_or_else(|| match ext {
            Some(e) => ApiError::Unsupported(format!("unrecognized extension '.{e}'")),
            None => ApiError::Bad(format!(
                "cannot determine the format of '{file_name}': no file extension \
                 and no recognized Content-Type"
            )),
        })?;
    let stem = std::path::Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document")
        .to_string();
    Ok(SourceDocument::from_bytes(stem, format, bytes))
}

/// Map an HTTP `Content-Type` (its media-type, parameters stripped) to an input
/// format — the common web types docling can convert. Anything else returns
/// `None` and the caller reports an unknown-format error.
fn format_from_content_type(content_type: &str) -> Option<InputFormat> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    Some(match mime.as_str() {
        "text/html" | "application/xhtml+xml" => InputFormat::Html,
        "application/pdf" => InputFormat::Pdf,
        "text/markdown" | "text/plain" => InputFormat::Md,
        "text/csv" => InputFormat::Csv,
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            InputFormat::Docx
        }
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            InputFormat::Pptx
        }
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => InputFormat::Xlsx,
        "application/epub+zip" => InputFormat::Epub,
        "image/jpeg" | "image/png" | "image/tiff" | "image/bmp" | "image/webp" => {
            InputFormat::Image
        }
        // SVG (#212) is its own format, not Image: the ML build rasterizes it
        // first, and OCR-less builds extract its <text> elements instead.
        "image/svg+xml" => InputFormat::Svg,
        // Upstream's FormatToMimeType for AUDIO and VIDEO (docling v2.114).
        "audio/wav" | "audio/x-wav" | "audio/mpeg" | "audio/mp3" | "audio/mp4" | "audio/m4a"
        | "audio/aac" | "audio/ogg" | "audio/flac" | "audio/x-flac" => InputFormat::Audio,
        "video/mp4" | "video/avi" | "video/x-msvideo" | "video/quicktime" | "video/x-matroska"
        | "video/webm" => InputFormat::Video,
        _ => return None,
    })
}

/// Largest URL-fetch response accepted (256 MiB default). Unlike the
/// request-body limit, `read_to_end` on a fetched response is otherwise
/// unbounded — a hostile URL streaming an endless body would exhaust memory.
/// Override with `DOCLING_RS_MAX_FETCH_BYTES`.
fn max_fetch_bytes() -> u64 {
    docling_core::env::parse("DOCLING_RS_MAX_FETCH_BYTES").unwrap_or(256 * 1024 * 1024)
}

/// Escape hatch for local development: when `DOCLING_RS_ALLOW_PRIVATE_IP_FETCH`
/// is set to a truthy value, the SSRF IP block-list is not enforced, so a URL
/// like `http://localhost:8080/doc.pdf` can be fetched. Off by default —
/// leave it unset in production.
fn allow_private_ip_fetch() -> bool {
    docling_core::env::flag("DOCLING_RS_ALLOW_PRIVATE_IP_FETCH")
}

/// Reject a resolved IP that points back into the local host or infrastructure.
/// This is the core SSRF guard: without it, `{"url": "http://169.254.169.254/…"}`
/// or `http://127.0.0.1:…` would let a caller reach cloud metadata and internal
/// services from the server's network position.
fn is_blocked_ip(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                // Carrier-grade NAT 100.64.0.0/10.
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                // Unique-local fc00::/7 and link-local fe80::/10.
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                // IPv4-mapped (::ffff:a.b.c.d): re-check the embedded v4.
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| is_blocked_ip(IpAddr::V4(v4)))
        }
    }
}

/// Fetch a URL input (blocking; run on the blocking pool). The name comes
/// from `file_name` or the URL path's last segment.
fn fetch_url(url: &str, file_name: Option<&str>) -> Result<SourceDocument, ApiError> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ApiError::Bad(format!("unsupported URL scheme in '{url}'")));
    }
    // SSRF guard: resolve the host and reject if it maps to a private/loopback/
    // link-local address, and forbid redirects (a public URL could 30x-bounce
    // to an internal target, defeating this pre-check). This is a best-effort
    // mitigation — a DNS-rebinding race between this resolution and ureq's own
    // connect remains theoretically possible; the deployment-level control is
    // to leave URL fetch disabled unless the network is trusted.
    let parsed =
        url::Url::parse(url).map_err(|e| ApiError::Bad(format!("bad URL '{url}': {e}")))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| ApiError::Bad(format!("no host in URL '{url}'")))?;
    let port = parsed.port_or_known_default().unwrap_or(80);
    let mut resolved = (host, port)
        .to_socket_addrs()
        .map_err(|e| ApiError::Bad(format!("cannot resolve {host}: {e}")))?
        .peekable();
    if resolved.peek().is_none() {
        return Err(ApiError::Bad(format!("cannot resolve {host}")));
    }
    if !allow_private_ip_fetch() {
        for addr in resolved {
            if is_blocked_ip(addr.ip()) {
                return Err(ApiError::Bad(format!(
                    "refusing to fetch {url}: resolves to a private/loopback address \
                     (set DOCLING_RS_ALLOW_PRIVATE_IP_FETCH=1 for local development)"
                )));
            }
        }
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .build()
        .into();
    let mut response = agent
        .get(url)
        .call()
        .map_err(|e| ApiError::Bad(format!("fetching {url}: {e}")))?;
    // Kept for format detection when the URL/name has no usable extension.
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let max_fetch = max_fetch_bytes();
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(max_fetch + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| ApiError::Bad(format!("reading {url}: {e}")))?;
    if bytes.len() as u64 > max_fetch {
        return Err(ApiError::Bad(format!(
            "response from {url} exceeds {max_fetch} bytes"
        )));
    }
    let name = file_name
        .map(|s| s.to_string())
        .or_else(|| {
            url.split('/')
                .next_back()
                .map(|s| s.split(['?', '#']).next().unwrap_or(s).to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "document".to_string());
    // Record the fetch URL as the document's base URL so relative `<img src>`
    // on a fetched web page resolve against its origin when fetch_images is on.
    Ok(source_from_named_bytes_ct(&name, bytes, content_type.as_deref())?.with_base_url(url))
}

/// Convert to a [`DoclingDocument`], routing PDF/image through the warm
/// pipeline and everything else through the declarative converter.
fn convert_document(
    state: &AppState,
    source: SourceDocument,
    options: &ConvertOptions,
) -> Result<DoclingDocument, ApiError> {
    match source.format {
        InputFormat::Pdf | InputFormat::Image => {
            // Recover from a poisoned lock instead of propagating the panic: a
            // single crafted PDF/image that panics inside `convert` below drops
            // the guard mid-unwind and poisons the mutex. Without this recovery
            // every later request would panic on `.lock().unwrap()` too, turning
            // one bad document into a permanent outage of this endpoint. The
            // pipeline state is rebuilt/validated by `warm_pipeline`, so reusing
            // it after a panic is safe.
            let mut guard = state
                .pipeline
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let pipeline = warm_pipeline(&mut guard, options)?;
            // The page window (#80) is per-request configuration on the shared
            // warm pipeline — set it unconditionally so no request inherits a
            // previous one's window. Images are single-page; no window.
            let range = options
                .pages
                .as_deref()
                .map(docling::parse_page_range)
                .transpose()
                .map_err(|e| ApiError::Bad(format!("pages: {e}")))?;
            pipeline.set_pages(range);
            // OCR language likewise applies per request; only a worker whose
            // cached recognition model mismatches actually reloads anything.
            pipeline.set_ocr_lang(parse_ocr_lang(options.ocr_lang.as_deref())?);
            let doc = match source.format {
                InputFormat::Pdf => pipeline.convert(&source.bytes, None, &source.name),
                _ => pipeline.convert_image(&source.bytes, &source.name),
            }
            .map_err(|e| ApiError::Internal(e.to_string()))?;
            Ok(doc)
        }
        _ => {
            let converter = request_converter(state, options)?;
            converter
                .convert(source)
                .map(|r| r.document)
                .map_err(|e| ApiError::Unsupported(e.to_string()))
        }
    }
}

/// The lazily-loaded warm pipeline. Pipeline switches (`no_ocr`,
/// `no_table_former`) are per-instance, so a request that changes them
/// rebuilds the pipeline (model sessions reload); steady-state traffic with
/// stable options keeps the warm one.
fn warm_pipeline<'a>(
    slot: &'a mut Option<Pipeline>,
    options: &ConvertOptions,
) -> Result<&'a mut Pipeline, ApiError> {
    let no_ocr = options.no_ocr.unwrap_or(false);
    let no_tf = options.no_table_former.unwrap_or(false);
    let ntp = options.no_text_panels.unwrap_or(false);
    if no_ocr || no_tf || ntp {
        let p = Pipeline::new()
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .no_ocr(no_ocr)
            .no_table_former(no_tf)
            .no_text_panels(ntp);
        *slot = Some(p);
    } else if slot.is_none() {
        *slot = Some(Pipeline::new().map_err(|e| ApiError::Internal(e.to_string()))?);
    }
    Ok(slot.as_mut().expect("just filled"))
}

/// Per-request declarative converter (construction is cheap — it's
/// configuration, models don't apply).
fn request_converter(
    state: &AppState,
    options: &ConvertOptions,
) -> Result<DocumentConverter, ApiError> {
    let mut converter = DocumentConverter::new()
        .strict(options.strict.unwrap_or(state.cfg.strict))
        // `fetch_images` pulls external `<img src>` over the network — the same
        // outbound-fetch / SSRF surface as URL inputs, so it lives behind the
        // same `--allow-url-fetch` gate. Off by default, it's silently ignored
        // rather than honored (the UI greys the box; an API caller just gets
        // placeholder images instead of a surprise outbound fetch).
        .fetch_images(state.cfg.allow_url_fetch && options.fetch_images.unwrap_or(false))
        .asr_model(options.asr_model.clone())
        .asr_lang(options.asr_lang.clone())
        .video_frames(
            options
                .video_frames
                .unwrap_or(docling::DEFAULT_VIDEO_FRAMES),
        )
        .no_ocr(options.no_ocr.unwrap_or(false))
        .force_full_page_ocr(options.force_full_page_ocr.unwrap_or(false))
        .no_table_former(options.no_table_former.unwrap_or(false))
        .no_text_panels(options.no_text_panels.unwrap_or(false));
    if let Some(pages) = &options.pages {
        let (first, last) =
            docling::parse_page_range(pages).map_err(|e| ApiError::Bad(format!("pages: {e}")))?;
        converter = converter.page_range(first, last);
    }
    if parse_ocr_lang(options.ocr_lang.as_deref())?.is_some() {
        converter = converter.ocr_lang(options.ocr_lang.clone().expect("checked above"));
    }
    Ok(converter)
}

/// Validate a request's `ocr_lang` (None passes through — the engine default).
fn parse_ocr_lang(raw: Option<&str>) -> Result<Option<docling::OcrLang>, ApiError> {
    raw.map(|v| {
        docling::OcrLang::parse(v)
            .ok_or_else(|| ApiError::Bad(format!("ocr_lang {v:?} is not en|ch")))
    })
    .transpose()
}

/// Markdown response: converted through the streaming serializer, body sent
/// chunked as pages finish. The semaphore permit moves into the worker so the
/// slot stays held until the stream ends.
async fn stream_markdown(
    state: Arc<AppState>,
    source: SourceDocument,
    options: ConvertOptions,
    image_mode: ImageMode,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<Response, ApiError> {
    // A chunk is its text plus (first chunk of a pipeline conversion only) the
    // confidence summary header — computable only after conversion, which is
    // exactly when the PDF/image branch sends its single chunk.
    type Chunk = (String, Option<header::HeaderValue>);
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Chunk, String>>(8);
    let st = state.clone();
    tokio::task::spawn_blocking(move || {
        // Held until this worker (and thus the response body) is done.
        let _permit = permit;
        let send = |item: Result<Chunk, String>| {
            // The receiver disappearing means the client went away — stop.
            tx.blocking_send(item).is_ok()
        };
        match source.format {
            InputFormat::Pdf | InputFormat::Image => {
                // Buffered document → streamed serialization is pointless for
                // images (one step); PDFs stream page by page through the
                // warm pipeline's converter equivalent: convert, then stream
                // the serializer output. (True page-by-page pipeline
                // streaming holds the model mutex anyway, so the wall-clock
                // is the same; the client still gets incremental output.)
                match convert_document(&st, source, &options) {
                    Ok(mut doc) => {
                        doc.strict_markdown = options.strict.unwrap_or(st.cfg.strict);
                        let md = match image_mode {
                            ImageMode::Placeholder => doc.export_to_markdown(),
                            _ => {
                                doc.export_to_markdown_with_images(image_mode, "artifacts")
                                    .0
                            }
                        };
                        send(Ok((md, confidence_header(&doc))));
                    }
                    Err(e) => {
                        send(Err(api_error_message(e)));
                    }
                }
            }
            _ => {
                let converter = match request_converter(&st, &options) {
                    Ok(c) => c,
                    Err(e) => {
                        send(Err(api_error_message(e)));
                        return;
                    }
                };
                match converter.convert_streaming_images(source, image_mode) {
                    Ok(stream) => {
                        for chunk in stream {
                            match chunk {
                                Ok(s) => {
                                    if !send(Ok((s, None))) {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    send(Err(e.to_string()));
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        send(Err(e.to_string()));
                    }
                }
            }
        }
    });

    // First chunk decides the status code; later errors abort the stream
    // mid-body (the client sees a truncated response).
    let mut rx = rx;
    let first = rx.recv().await;
    match first {
        // No chunks means the document converted to empty Markdown (e.g. an
        // HTML page with no extractable content) — a valid result, not a
        // server error. Return an empty 200 body rather than a 500.
        None => Ok((
            [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
            Body::empty(),
        )
            .into_response()),
        Some(Err(e)) => Err(ApiError::Unsupported(e)),
        Some(Ok((first_chunk, confidence))) => {
            use tokio_stream::StreamExt;
            let rest = tokio_stream::wrappers::ReceiverStream::new(rx);
            let stream = tokio_stream::once(Ok((first_chunk, None)))
                .chain(rest)
                .map(|item| {
                    item.map(|(text, _)| text.into_bytes()).map_err(|e| {
                        std::io::Error::other(format!("conversion failed mid-stream: {e}"))
                    })
                });
            let mut response = (
                [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
                Body::from_stream(stream),
            )
                .into_response();
            if let Some(value) = confidence {
                response.headers_mut().insert("x-docling-confidence", value);
            }
            Ok(response)
        }
    }
}

fn api_error_message(e: ApiError) -> String {
    api_error_parts(e).1
}

#[cfg(test)]
mod ssrf_tests {
    use super::is_blocked_ip;
    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn blocks_internal_targets() {
        // Loopback, private ranges, link-local (incl. cloud metadata),
        // unspecified, CGNAT, and the IPv4-mapped IPv6 forms must all be
        // refused as SSRF targets.
        for s in [
            "127.0.0.1",
            "127.5.5.5",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254", // AWS/GCP metadata
            "0.0.0.0",
            "100.64.0.1", // carrier-grade NAT
            "::1",
            "fe80::1",          // link-local
            "fc00::1",          // unique-local
            "::ffff:127.0.0.1", // IPv4-mapped loopback
            "::ffff:169.254.169.254",
        ] {
            assert!(is_blocked_ip(ip(s)), "{s} should be blocked");
        }
    }

    #[test]
    fn allows_public_targets() {
        for s in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "2606:4700:4700::1111",
        ] {
            assert!(!is_blocked_ip(ip(s)), "{s} should be allowed");
        }
    }

    #[test]
    fn url_fetch_off_by_default() {
        assert!(!super::ServeConfig::default().allow_url_fetch);
    }

    #[test]
    fn content_type_maps_to_format_when_extension_missing() {
        use super::{source_from_named_bytes_ct, ApiError, InputFormat};
        // `ApiError` has no `Debug`, so match rather than `.expect()`.
        let fmt = |r: Result<super::SourceDocument, ApiError>| r.ok().map(|s| s.format);

        // A URL with no extension (iana example) resolves via Content-Type.
        assert_eq!(
            fmt(source_from_named_bytes_ct(
                "example-domains",
                b"<html></html>".to_vec(),
                Some("text/html; charset=utf-8"),
            )),
            Some(InputFormat::Html)
        );
        // A usable extension still wins over the Content-Type.
        assert_eq!(
            fmt(source_from_named_bytes_ct(
                "a.pdf",
                b"%PDF".to_vec(),
                Some("text/html"),
            )),
            Some(InputFormat::Pdf)
        );
        // Neither an extension nor a known Content-Type → a 4xx (Bad), not a 500.
        assert!(matches!(
            source_from_named_bytes_ct("noext", b"x".to_vec(), Some("application/octet-stream")),
            Err(ApiError::Bad(_))
        ));
    }

    #[test]
    fn private_ip_escape_hatch_defaults_off() {
        // The env var gates only development use; unset it must read as false
        // so the block-list is enforced by default. (Set within this test only,
        // then cleared, to avoid leaking to sibling tests.)
        std::env::remove_var("DOCLING_RS_ALLOW_PRIVATE_IP_FETCH");
        assert!(!super::allow_private_ip_fetch());
        for (val, want) in [
            ("1", true),
            ("true", true),
            ("0", false),
            ("false", false),
            ("", false),
        ] {
            std::env::set_var("DOCLING_RS_ALLOW_PRIVATE_IP_FETCH", val);
            assert_eq!(super::allow_private_ip_fetch(), want, "value {val:?}");
        }
        std::env::remove_var("DOCLING_RS_ALLOW_PRIVATE_IP_FETCH");
    }
}
