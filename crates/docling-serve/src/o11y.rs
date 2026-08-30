//! Observability (#297): structured request logging, Prometheus metrics, and
//! optional OpenTelemetry trace export — the Rust counterpart of Python
//! docling-serve's OTel integration, with the same posture: Prometheus-style
//! metrics on by default, OTLP traces opt-in.
//!
//! Three layers, in increasing weight:
//!
//! 1. **Structured logs** (always on): every request runs inside a `tracing`
//!    span and emits one `info` event with method, path, status and latency.
//!    `RUST_LOG` filters (default `info`); output goes to stderr next to the
//!    existing startup diagnostics.
//! 2. **`GET /metrics`** (always compiled, no dependencies): Prometheus text
//!    exposition of request counts/latency, in-flight gauge, and per-outcome
//!    conversion counts — hand-rolled counters like the rest of this
//!    workspace's lean-dependency choices. `/metrics`, `/health` and
//!    `/ready` probes are not counted (mirroring Python docling-serve's
//!    health-endpoint sampling exclusion).
//! 3. **OTLP trace export** (cargo feature `otel`, runtime-gated): with the
//!    feature compiled in *and* `OTEL_EXPORTER_OTLP_ENDPOINT` set, the same
//!    request spans ship over OTLP/gRPC. Service name comes from
//!    `OTEL_SERVICE_NAME` (default `docling-serve`, matching Python's
//!    `otel_service_name`). Without the env var the feature is inert.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Instant;

use axum::body::Body;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Latency histogram buckets (seconds). Conversions run from milliseconds
/// (declarative) to minutes (large PDFs through the ML pipeline), so the
/// buckets stretch far right of a typical web service's.
const BUCKETS: [f64; 12] = [
    0.005, 0.025, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 15.0, 60.0, 300.0, 900.0,
];

#[derive(Default)]
struct Histogram {
    buckets: [AtomicU64; 12],
    count: AtomicU64,
    /// Sum in microseconds — atomics can't hold floats, and µs keeps a year
    /// of accumulated latency well inside u64.
    sum_micros: AtomicU64,
}

impl Histogram {
    fn observe(&self, seconds: f64) {
        for (i, b) in BUCKETS.iter().enumerate() {
            if seconds <= *b {
                self.buckets[i].fetch_add(1, Ordering::Relaxed);
            }
        }
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_micros
            .fetch_add((seconds * 1e6) as u64, Ordering::Relaxed);
    }

    fn render(&self, out: &mut String, name: &str) {
        use std::fmt::Write;
        for (i, b) in BUCKETS.iter().enumerate() {
            let _ = writeln!(
                out,
                "{name}_bucket{{le=\"{b}\"}} {}",
                self.buckets[i].load(Ordering::Relaxed)
            );
        }
        let _ = writeln!(
            out,
            "{name}_bucket{{le=\"+Inf\"}} {}",
            self.count.load(Ordering::Relaxed)
        );
        // No label braces on _sum/_count: an empty `{}` is invalid OpenMetrics.
        let _ = writeln!(
            out,
            "{name}_sum {}",
            self.sum_micros.load(Ordering::Relaxed) as f64 / 1e6
        );
        let _ = writeln!(out, "{name}_count {}", self.count.load(Ordering::Relaxed));
    }
}

/// The process-wide metrics registry. Label cardinality is deliberately tiny
/// and fixed — status *class* (2xx/4xx/5xx), not per-path/per-status — so a
/// scraper can never be blown up by request-derived label values.
#[derive(Default)]
pub struct Metrics {
    requests_2xx: AtomicU64,
    requests_4xx: AtomicU64,
    requests_5xx: AtomicU64,
    in_flight: AtomicI64,
    latency: Histogram,
    conversions_success: AtomicU64,
    conversions_failure: AtomicU64,
}

fn metrics() -> &'static Metrics {
    static M: std::sync::OnceLock<Metrics> = std::sync::OnceLock::new();
    M.get_or_init(Metrics::default)
}

/// Record a finished conversion item (one per document — a batch counts each
/// item). Called from the conversion paths, not the HTTP layer, so async
/// jobs count when they *run*, not when they're submitted.
pub fn record_conversion(success: bool) {
    let m = metrics();
    if success {
        m.conversions_success.fetch_add(1, Ordering::Relaxed);
    } else {
        m.conversions_failure.fetch_add(1, Ordering::Relaxed);
    }
}

/// Endpoints excluded from request metrics and request logs: scrapes and
/// liveness probes would otherwise dominate every counter (and, under OTel,
/// every trace) — the same exclusion Python docling-serve applies to its
/// health endpoint.
fn is_probe(path: &str) -> bool {
    matches!(path, "/metrics" | "/health" | "/ready")
}

/// Axum middleware: wrap the request in a `tracing` span, log its outcome,
/// and record the metrics above.
pub async fn track(req: Request<Body>, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    if is_probe(&path) {
        return next.run(req).await;
    }
    let m = metrics();
    m.in_flight.fetch_add(1, Ordering::Relaxed);
    let started = Instant::now();
    let span = tracing::info_span!("request", %method, path = %path);
    let response = {
        let _enter = span.enter();
        next.run(req).await
    };
    let elapsed = started.elapsed().as_secs_f64();
    m.in_flight.fetch_sub(1, Ordering::Relaxed);
    let status = response.status();
    match status.as_u16() {
        200..=399 => m.requests_2xx.fetch_add(1, Ordering::Relaxed),
        400..=499 => m.requests_4xx.fetch_add(1, Ordering::Relaxed),
        _ => m.requests_5xx.fetch_add(1, Ordering::Relaxed),
    };
    m.latency.observe(elapsed);
    tracing::info!(
        parent: &span,
        status = status.as_u16(),
        elapsed_ms = (elapsed * 1e3) as u64,
        "request"
    );
    response
}

/// `GET /metrics` — Prometheus text exposition (version 0.0.4).
pub async fn metrics_endpoint() -> Response {
    use std::fmt::Write;
    let m = metrics();
    let mut out = String::with_capacity(2048);
    let _ = writeln!(
        out,
        "# HELP docling_serve_requests_total HTTP requests handled, by status class.\n\
         # TYPE docling_serve_requests_total counter"
    );
    for (class, v) in [
        ("2xx", &m.requests_2xx),
        ("4xx", &m.requests_4xx),
        ("5xx", &m.requests_5xx),
    ] {
        let _ = writeln!(
            out,
            "docling_serve_requests_total{{class=\"{class}\"}} {}",
            v.load(Ordering::Relaxed)
        );
    }
    let _ = writeln!(
        out,
        "# HELP docling_serve_requests_in_flight Requests currently being handled.\n\
         # TYPE docling_serve_requests_in_flight gauge\n\
         docling_serve_requests_in_flight {}",
        m.in_flight.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "# HELP docling_serve_request_duration_seconds End-to-end request latency.\n\
         # TYPE docling_serve_request_duration_seconds histogram"
    );
    m.latency
        .render(&mut out, "docling_serve_request_duration_seconds");
    let _ = writeln!(
        out,
        "# HELP docling_serve_conversions_total Converted documents, by outcome (batch items count individually; async jobs count when they run).\n\
         # TYPE docling_serve_conversions_total counter"
    );
    for (outcome, v) in [
        ("success", &m.conversions_success),
        ("failure", &m.conversions_failure),
    ] {
        let _ = writeln!(
            out,
            "docling_serve_conversions_total{{outcome=\"{outcome}\"}} {}",
            v.load(Ordering::Relaxed)
        );
    }
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        out,
    )
        .into_response()
}

/// Install the global `tracing` subscriber: an env-filtered (`RUST_LOG`,
/// default `info`) stderr logger, plus — with the `otel` cargo feature and
/// `OTEL_EXPORTER_OTLP_ENDPOINT` set — an OTLP span exporter. Idempotent and
/// quiet when a subscriber already exists (tests, embedding applications).
pub fn init() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let fmt = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let registry = tracing_subscriber::registry().with(filter).with(fmt);

    #[cfg(feature = "otel")]
    {
        if let Some(exporter) = otel_layer() {
            let _ = registry.with(exporter).try_init();
            return;
        }
    }
    let _ = registry.try_init();
}

/// Flush and stop the OTLP exporter (batched spans would otherwise be lost on
/// SIGTERM). A no-op without the `otel` feature or with export inactive.
pub fn shutdown() {
    #[cfg(feature = "otel")]
    if let Some(provider) = OTEL_PROVIDER.get() {
        if let Err(e) = provider.shutdown() {
            eprintln!("docling-serve: OTLP shutdown flush failed: {e}");
        }
    }
}

/// The live tracer provider, kept for the graceful-shutdown flush above.
#[cfg(feature = "otel")]
static OTEL_PROVIDER: std::sync::OnceLock<opentelemetry_sdk::trace::SdkTracerProvider> =
    std::sync::OnceLock::new();

/// The OTLP export layer, when configured. `None` without
/// `OTEL_EXPORTER_OTLP_ENDPOINT` — compiling the feature in must not change
/// behavior until the operator points it somewhere.
#[cfg(feature = "otel")]
fn otel_layer<S>(
) -> Option<tracing_opentelemetry::OpenTelemetryLayer<S, opentelemetry_sdk::trace::Tracer>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    use opentelemetry::trace::TracerProvider as _;

    docling_core::env::nonempty("OTEL_EXPORTER_OTLP_ENDPOINT")?;
    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("docling-serve: OTLP exporter init failed ({e}); traces disabled");
            return None;
        }
    };
    let service = docling_core::env::nonempty("OTEL_SERVICE_NAME")
        .unwrap_or_else(|| "docling-serve".to_string());
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            opentelemetry_sdk::Resource::builder()
                .with_service_name(service)
                .build(),
        )
        .build();
    let tracer = provider.tracer("docling-serve");
    let _ = OTEL_PROVIDER.set(provider.clone());
    opentelemetry::global::set_tracer_provider(provider);
    eprintln!("docling-serve: OTLP trace export enabled (OTEL_EXPORTER_OTLP_ENDPOINT)");
    Some(tracing_opentelemetry::layer().with_tracer(tracer))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe exclusion (scrapes and liveness checks must not dominate the
    /// counters) is exact-match on the three fixed paths — API traffic that
    /// merely resembles them still counts.
    #[test]
    fn probe_paths_are_excluded_exactly() {
        for p in ["/metrics", "/health", "/ready"] {
            assert!(is_probe(p), "{p} is a probe");
        }
        for p in ["/", "/v1/convert", "/healthz", "/metrics/x", "/ready2"] {
            assert!(!is_probe(p), "{p} is API traffic");
        }
    }

    /// Cumulative-bucket semantics: an observation lands in every bucket at or
    /// above its value, `+Inf` equals the count, and the sum round-trips
    /// through the microsecond storage.
    #[test]
    fn histogram_buckets_are_cumulative() {
        let h = Histogram::default();
        h.observe(0.05); // > 0.025, ≤ 0.1
        h.observe(30.0); // > 15, ≤ 60
        let mut out = String::new();
        h.render(&mut out, "t");
        assert!(out.contains("t_bucket{le=\"0.025\"} 0"));
        assert!(out.contains("t_bucket{le=\"0.1\"} 1"));
        assert!(out.contains("t_bucket{le=\"15\"} 1"));
        assert!(out.contains("t_bucket{le=\"60\"} 2"));
        assert!(out.contains("t_bucket{le=\"+Inf\"} 2"));
        assert!(out.contains("t_count 2"));
        assert!(out.contains("t_sum 30.05"));
    }
}
