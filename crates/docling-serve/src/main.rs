//! `docling-serve` — standalone binary for the HTTP conversion API.
//!
//! Usage: docling-serve [--addr HOST:PORT] [--concurrency N] [--max-body-mb N]
//!                      [--queue-size N] [--result-ttl SECS] [--max-memory-mb N]
//!                      [--warmup] [--allow-url-fetch] [--strict]
//!
//!   --addr HOST:PORT  bind address (default: 127.0.0.1:5001). Bind 0.0.0.0
//!                     only behind a trusted proxy.
//!   --concurrency N   max conversions in flight; excess requests queue
//!                     (default: 2)
//!   --max-body-mb N   request body cap for uploads, in MiB (default: 256)
//!   --queue-size N    max async jobs (#182) queued/unfetched at once; further
//!                     submissions get 429 (default: 16)
//!   --result-ttl SECS how long a finished async job's result stays fetchable
//!                     (default: 600)
//!   --warmup          load the PDF/image models at startup; /ready returns
//!                     503 until they are loaded
//!   --allow-url-fetch accept {"url": …} inputs (outbound fetch — SSRF surface;
//!                     off by default). A private/loopback/link-local IP guard
//!                     applies even when enabled.
//!   --no-url-fetch    accepted for compatibility (URL fetch is now off by
//!                     default; this is a no-op)
//!   --strict          default to the cleaner strict Markdown dialect

use std::process::ExitCode;

use docling_serve::{serve, ServeConfig};

fn main() -> ExitCode {
    // #263: a long-lived server defaults the ONNX CPU arena OFF — measured
    // here, a warm server's retained RSS drops ~3x (2.0 GB -> 0.7 GB after
    // large-PDF requests) at no measurable latency cost, and stops ratcheting
    // with every new page shape. Explicit DOCLING_RS_NO_ARENA=0 restores the
    // arena. Set before any session loads; the process is single-threaded
    // this early.
    if std::env::var_os("DOCLING_RS_NO_ARENA").is_none() {
        std::env::set_var("DOCLING_RS_NO_ARENA", "1");
    }

    let mut cfg = ServeConfig::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--addr" => match args.next() {
                Some(v) => cfg.addr = v,
                None => return usage("--addr needs HOST:PORT"),
            },
            "--concurrency" => match args.next().and_then(|v| v.parse().ok()) {
                Some(v) if v >= 1 => cfg.concurrency = v,
                _ => return usage("--concurrency needs a positive integer"),
            },
            "--max-body-mb" => match args.next().and_then(|v| v.parse::<usize>().ok()) {
                Some(v) if v >= 1 => cfg.max_body_bytes = v * 1024 * 1024,
                _ => return usage("--max-body-mb needs a positive integer"),
            },
            "--queue-size" => match args.next().and_then(|v| v.parse().ok()) {
                Some(v) if v >= 1 => cfg.queue_size = v,
                _ => return usage("--queue-size needs a positive integer"),
            },
            "--result-ttl" => match args.next().and_then(|v| v.parse().ok()) {
                Some(v) if v >= 1 => cfg.result_ttl_secs = v,
                _ => return usage("--result-ttl needs a positive number of seconds"),
            },
            // #263: memory ceiling for admission control. 0 disables; unset =
            // auto-detect the container's cgroup limit.
            "--max-memory-mb" => match args.next().and_then(|v| v.parse().ok()) {
                Some(v) => cfg.max_memory_mb = Some(v),
                None => return usage("--max-memory-mb needs a number (0 disables)"),
            },
            "--warmup" => cfg.warmup = true,
            "--allow-url-fetch" => cfg.allow_url_fetch = true,
            // URL fetch is off by default now; keep the old flag as a no-op so
            // existing invocations don't break.
            "--no-url-fetch" => cfg.allow_url_fetch = false,
            "--strict" => cfg.strict = true,
            "--help" | "-h" => return usage(""),
            other => return usage(&format!("unknown argument '{other}'")),
        }
    }

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(serve(cfg)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage(err: &str) -> ExitCode {
    if !err.is_empty() {
        eprintln!("error: {err}");
    }
    eprintln!(
        "usage: docling-serve [--addr HOST:PORT] [--concurrency N] [--max-body-mb N] [--queue-size N] [--result-ttl SECS] [--max-memory-mb N] [--warmup] [--allow-url-fetch] [--strict]"
    );
    if err.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    }
}
