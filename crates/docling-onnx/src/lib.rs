//! Execution-provider selection for the ONNX sessions (#74, #288).
//!
//! Shared by every crate that owns `ort` sessions (docling-pdf's layout/
//! TableFormer/OCR/enrichment models, docling-asr's Whisper encoder/decoder,
//! docling-rag's embedder) so a single env var switches the whole process.
//!
//! CPU is the default and the only provider in a default build — the GPU
//! providers exist behind cargo features (`cuda`, `tensorrt`, `directml`,
//! `coreml`; plus the CPU-class `xnnpack`, #324) so the standard build keeps
//! zero GPU dependencies. A feature only
//! *compiles* a provider in (it makes `ort` link/download an ONNX Runtime
//! binary that contains that EP); which provider actually runs is chosen at
//! startup from `DOCLING_RS_EP`:
//!
//! * unset — `auto` in a build that compiled any GPU provider in (you chose
//!   a GPU build or installed the GPU wheel: use the GPU when one is usable,
//!   fall back to CPU when not); plain CPU in a default build
//! * `cpu` — force CPU, exactly the pre-#74 behavior
//! * `cuda` / `tensorrt` (`trt`) / `directml` (`dml`) / `coreml` /
//!   `xnnpack` — that provider, registered with error-on-failure: an *explicitly requested*
//!   accelerator that can't initialize (missing driver, no device) fails the
//!   session load loudly instead of silently degrading to a CPU run that
//!   looks fine but is 10× slower than expected. Requesting a provider the
//!   binary wasn't compiled with warns once and stays on CPU (there is
//!   nothing to register at all in that case).
//! * `auto` — every compiled-in provider is registered in performance order
//!   (TensorRT, CUDA, CoreML, DirectML, then XNNPACK) and ONNX Runtime falls
//!   back down the list — ultimately to CPU — at session creation. The "try GPU if there is
//!   one" mode for images built once and deployed on mixed fleets.
//!
//! CoreML registers with the `MLProgram` model format by default (#324):
//! the ONNX Runtime default, `NeuralNetwork`, cannot place operators the
//! layout model carries (`GridSample`, `ScatterND`, dynamic output shapes)
//! and aborts inference on Apple silicon instead of falling back.
//! `DOCLING_RS_COREML_FORMAT=neuralnetwork` restores the old format for
//! pre-macOS-12 systems. Two safety defaults ride along (issue-#324
//! testing, M4 Max): only *static-shaped* partitions are handed to CoreML
//! (`DOCLING_RS_COREML_STATIC_SHAPES=0` opts out) — dynamic partitions under
//! MLProgram fail an MPSGraph assertion as an uncatchable SIGABRT — and
//! compute units default to `cpu_and_gpu` rather than `all`
//! (`DOCLING_RS_COREML_UNITS`: `all`|`cpu_and_gpu`|`cpu_and_ne`|`cpu_only`),
//! since the fp16 Neural Engine silently corrupts this model's logits.
//! `DOCLING_RS_XNNPACK_THREADS` sizes XNNPACK's own thread pool.
//!
//! The int8 model defaults in docling-pdf are skipped whenever a GPU provider
//! is selected ([`prefers_fp32`]): the int8 exports are QDQ graphs calibrated
//! for CPU kernels — on GPU they only add de-quantize traffic and were never
//! conformance-validated there, while fp32 is (see docs/PDF_CONFORMANCE.md).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ort::ep::ExecutionProviderDispatch;
use ort::session::builder::{GraphOptimizationLevel, SessionBuilder};
use ort::session::Session;

/// The parsed `DOCLING_RS_EP` choice. Named GPU variants are only ever
/// *selected* (returned by [`choice`]) when their cargo feature is compiled
/// in; [`parse`] itself is feature-blind so it can be unit-tested everywhere.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ep {
    Cpu,
    Cuda,
    TensorRt,
    DirectMl,
    CoreMl,
    /// CPU-class EP with ARM NEON / x86 SIMD kernels (#324) — an accelerator
    /// for machines without a usable GPU provider, Apple silicon included.
    Xnnpack,
    /// Register everything compiled in, let ONNX Runtime fall back.
    Auto,
}

/// Parse a `DOCLING_RS_EP` value. `None` for values that name no known
/// provider (the caller warns and stays on CPU).
pub fn parse(v: &str) -> Option<Ep> {
    match v.trim().to_ascii_lowercase().as_str() {
        "" | "cpu" => Some(Ep::Cpu),
        "cuda" => Some(Ep::Cuda),
        "tensorrt" | "trt" => Some(Ep::TensorRt),
        "directml" | "dml" => Some(Ep::DirectMl),
        "coreml" => Some(Ep::CoreMl),
        "xnnpack" => Some(Ep::Xnnpack),
        "auto" => Some(Ep::Auto),
        _ => None,
    }
}

/// Is this provider compiled into the binary (cargo feature enabled)?
fn compiled(ep: Ep) -> bool {
    match ep {
        Ep::Cpu => true,
        Ep::Cuda => cfg!(feature = "cuda"),
        Ep::TensorRt => cfg!(feature = "tensorrt"),
        Ep::DirectMl => cfg!(feature = "directml"),
        Ep::CoreMl => cfg!(feature = "coreml"),
        Ep::Xnnpack => cfg!(feature = "xnnpack"),
        Ep::Auto => true,
    }
}

fn any_gpu_compiled() -> bool {
    [Ep::Cuda, Ep::TensorRt, Ep::DirectMl, Ep::CoreMl]
        .into_iter()
        .any(compiled)
}

/// The choice when `DOCLING_RS_EP` is unset (or empty): a build that
/// compiled a GPU provider in defaults to `auto` — whoever built with
/// `--features cuda` (or installed the `docling-rs-cuda` wheel) wants the
/// GPU used when one is usable, and `auto`'s per-session registration
/// falls back to CPU when not. A default build has nothing to register and
/// stays on the exact pre-#74 CPU path.
fn default_choice() -> Ep {
    // XNNPACK counts here (whoever built with it wants it used) but not in
    // [`prefers_fp32`] — it runs the same CPU-calibrated graphs as the CPU EP.
    if any_gpu_compiled() || cfg!(feature = "xnnpack") {
        Ep::Auto
    } else {
        Ep::Cpu
    }
}

/// The effective provider choice for this process, resolved once. Invalid or
/// not-compiled-in requests degrade to CPU with a single stderr warning —
/// same convention as a missing model file.
pub fn choice() -> Ep {
    static CHOICE: OnceLock<Ep> = OnceLock::new();
    *CHOICE.get_or_init(|| {
        let Some(raw) = docling_core::env::nonempty("DOCLING_RS_EP") else {
            return default_choice();
        };
        let Some(ep) = parse(&raw) else {
            eprintln!(
                "docling-rs: DOCLING_RS_EP={raw:?} names no known execution provider \
                 (cpu|cuda|tensorrt|directml|coreml|xnnpack|auto); using CPU"
            );
            return Ep::Cpu;
        };
        if !compiled(ep) {
            eprintln!(
                "docling-rs: DOCLING_RS_EP={raw:?} requested, but this binary was built \
                 without that provider — rebuild with `--features {}`; using CPU",
                match ep {
                    Ep::Cuda => "cuda",
                    Ep::TensorRt => "tensorrt",
                    Ep::DirectMl => "directml",
                    Ep::CoreMl => "coreml",
                    Ep::Xnnpack => "xnnpack",
                    Ep::Cpu | Ep::Auto => unreachable!("always compiled"),
                }
            );
            return Ep::Cpu;
        }
        ep
    })
}

/// True when the int8 model defaults should be skipped in favor of fp32
/// because inference is (or may be) leaving the CPU. `Auto` counts as GPU as
/// soon as any GPU provider is compiled in: whether registration succeeds is
/// only known per-session, and a CPU fall-back running fp32 is merely the
/// pre-int8 speed, while a GPU running the CPU-calibrated int8 graph is a
/// conformance risk.
///
/// A no-op for ASR: the Whisper exports ship without int8 variants, so there
/// is no model selection for this to influence on that path.
pub fn prefers_fp32() -> bool {
    match choice() {
        // XNNPACK is CPU-class: the int8 QDQ graphs were calibrated for CPU
        // kernels and stay valid on it.
        Ep::Cpu | Ep::Xnnpack => false,
        Ep::Cuda | Ep::TensorRt | Ep::DirectMl | Ep::CoreMl => true,
        Ep::Auto => any_gpu_compiled(),
    }
}

/// The CoreML provider, configured from the environment (#324). `MLProgram`
/// is the default model format: ONNX Runtime's own default, `NeuralNetwork`,
/// cannot place operators the layout model carries (`GridSample`,
/// `ScatterND`, dynamic output shapes) and aborts inference with error -1 on
/// Apple silicon instead of falling back. `MLProgram` needs macOS 12+ —
/// older systems can restore the old format explicitly.
#[cfg(feature = "coreml")]
fn coreml_ep() -> ort::ep::CoreML {
    use ort::ep::coreml::{ComputeUnits, ModelFormat};
    static WARNED: OnceLock<()> = OnceLock::new();
    let format = match docling_core::env::nonempty("DOCLING_RS_COREML_FORMAT")
        .map(|v| v.to_ascii_lowercase())
        .as_deref()
    {
        None | Some("mlprogram") => ModelFormat::MLProgram,
        Some("neuralnetwork") => ModelFormat::NeuralNetwork,
        Some(other) => {
            let other = other.to_string();
            WARNED.get_or_init(|| {
                eprintln!(
                    "docling-rs: DOCLING_RS_COREML_FORMAT={other:?} is not                      mlprogram|neuralnetwork; using mlprogram"
                );
            });
            ModelFormat::MLProgram
        }
    };
    let mut ep = ort::ep::CoreML::default().with_model_format(format);
    // Static-shaped partitions only, ON by default (#324 follow-up): with the
    // stock dynamic-batch layout model, MLProgram otherwise fails an MPSGraph
    // assertion *inside* CoreML — a SIGABRT the process cannot catch, worse
    // than the NeuralNetwork error it replaced (M4 Max, macOS 26 report).
    // Keeping dynamic partitions off CoreML avoids it at the root;
    // `DOCLING_RS_COREML_STATIC_SHAPES=0` opts back into dynamic placement
    // for models known to be safe under it.
    let static_shapes =
        docling_core::env::nonempty("DOCLING_RS_COREML_STATIC_SHAPES").is_none_or(|v| {
            !matches!(
                v.to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        });
    if static_shapes {
        ep = ep.with_static_input_shapes(true);
    }
    // Compute units default to CPU+GPU, not ONNX Runtime's `ALL` (#324
    // follow-up): `ALL` may place partitions on the fp16 Neural Engine, which
    // silently corrupts this model's output (max|Δlogits| = 6.5 vs CPU, no
    // error raised) and measured slower than the GPU path. `all`/`cpu_and_ne`
    // remain available for models validated on the ANE.
    let units = match docling_core::env::nonempty("DOCLING_RS_COREML_UNITS")
        .map(|v| v.to_ascii_lowercase())
        .as_deref()
    {
        None | Some("cpu_and_gpu" | "cpu_gpu" | "gpu") => ComputeUnits::CPUAndGPU,
        Some("all") => ComputeUnits::All,
        Some("cpu_and_ne" | "cpu_ane" | "ane") => ComputeUnits::CPUAndNeuralEngine,
        Some("cpu_only" | "cpu") => ComputeUnits::CPUOnly,
        Some(other) => {
            let other = other.to_string();
            WARNED.get_or_init(|| {
                eprintln!(
                    "docling-rs: DOCLING_RS_COREML_UNITS={other:?} is not                      all|cpu_and_gpu|cpu_and_ne|cpu_only; using cpu_and_gpu"
                );
            });
            ComputeUnits::CPUAndGPU
        }
    };
    ep.with_compute_units(units)
}

/// The XNNPACK provider (#324). It runs its own thread pool;
/// `DOCLING_RS_XNNPACK_THREADS` sizes it, unset keeps ONNX Runtime's default.
#[cfg(feature = "xnnpack")]
fn xnnpack_ep() -> ort::ep::XNNPACK {
    let mut ep = ort::ep::XNNPACK::default();
    if let Some(n) = docling_core::env::parse::<usize>("DOCLING_RS_XNNPACK_THREADS")
        .and_then(core::num::NonZeroUsize::new)
    {
        ep = ep.with_intra_op_num_threads(n);
    }
    ep
}

/// The dispatch list for the current choice. `None` means "register nothing"
/// (CPU — leave the builder untouched, the pre-#74 code path).
// The cfg-gated pushes read as sequential to clippy in single-feature builds.
#[allow(clippy::vec_init_then_push)]
fn dispatches() -> Option<Vec<ExecutionProviderDispatch>> {
    // Since ort 2.0.0-rc.13 the `ort::ep::*` structs are themselves gated
    // behind their cargo features (rc.12 exposed them unconditionally), so
    // every construction site needs a cfg gate. `choice()` still guarantees a
    // named provider is only ever *selected* when compiled in — the
    // fallthrough arm below is unreachable at runtime and exists to keep the
    // match exhaustive in builds without that feature.
    let d = match choice() {
        Ep::Cpu => return None,
        #[cfg(feature = "cuda")]
        Ep::Cuda => vec![ort::ep::CUDA::default().build().error_on_failure()],
        #[cfg(feature = "tensorrt")]
        Ep::TensorRt => vec![ort::ep::TensorRT::default().build().error_on_failure()],
        #[cfg(feature = "directml")]
        Ep::DirectMl => vec![ort::ep::DirectML::default().build().error_on_failure()],
        #[cfg(feature = "coreml")]
        Ep::CoreMl => vec![coreml_ep().build().error_on_failure()],
        #[cfg(feature = "xnnpack")]
        Ep::Xnnpack => vec![xnnpack_ep().build().error_on_failure()],
        Ep::Auto => {
            #[allow(unused_mut)]
            let mut v: Vec<ExecutionProviderDispatch> = Vec::new();
            #[cfg(feature = "tensorrt")]
            v.push(ort::ep::TensorRT::default().build());
            #[cfg(feature = "cuda")]
            v.push(ort::ep::CUDA::default().build());
            #[cfg(feature = "coreml")]
            v.push(coreml_ep().build());
            #[cfg(feature = "directml")]
            v.push(ort::ep::DirectML::default().build());
            // Last, above only the implicit CPU fallback: a CPU-class
            // accelerator must not shadow a usable GPU.
            #[cfg(feature = "xnnpack")]
            v.push(xnnpack_ep().build());
            if v.is_empty() {
                return None; // CPU-only build: auto ≡ cpu
            }
            v
        }
        #[allow(unreachable_patterns)]
        ep => unreachable!("choice() returned {ep:?} without its cargo feature"),
    };
    Some(d)
}

/// Register the selected execution providers on a session builder. Called by
/// every ONNX session in the workspace; a no-op (and infallible) in the
/// default CPU configuration.
pub fn apply(builder: SessionBuilder) -> Result<SessionBuilder, String> {
    let Some(eps) = dispatches() else {
        // No GPU EP selected: the CPU fallback still honors the arena knob.
        return memory_opts(builder);
    };
    static LOGGED: OnceLock<()> = OnceLock::new();
    LOGGED.get_or_init(|| {
        if docling_core::env::flag("DOCLING_RS_TIMING") {
            eprintln!("docling-rs: execution providers: {eps:?}");
        }
        // CoreML's cost profile is invisible otherwise (#324 follow-up
        // testing): session creation runs ~2 s per worker, does not
        // parallelize, and only amortizes over long-lived processes or large
        // batches — a one-shot CLI conversion is typically *slower* than CPU,
        // with correct output and nothing else to hint at why. Say so once at
        // the moment the cost is incurred.
        #[cfg(feature = "coreml")]
        if matches!(choice(), Ep::CoreMl | Ep::Auto) {
            eprintln!(
                "docling-rs: CoreML registered; its session setup costs roughly 2 s per \
                 worker and pays off for long-lived processes and large batches — for \
                 short one-shot runs DOCLING_RS_EP=cpu is usually faster"
            );
        }
    });
    let builder = builder
        .with_execution_providers(eps)
        .map_err(|e| format!("execution provider registration: {e}"))?;
    // After the GPU EPs, so they keep registration priority.
    memory_opts(builder)
}

/// Session memory options (#263): with `DOCLING_RS_NO_ARENA` set, the CPU
/// execution provider registers with its memory arena disabled
/// (`DisableCpuMemArena`) and initializers stay out of arena allocations.
/// ONNX Runtime's CPU arena grows a slab for every new tensor shape it sees
/// — a PDF's pages all differ, so a warm server's arena ratchets up with the
/// largest documents it ever served and never returns a byte; without it,
/// activations free after each run (and `malloc_trim` hands them back to the
/// OS) at a few percent inference cost. Off by default — batch CLI runs
/// prefer the arena's speed. Called at the *end* of [`apply`], so an explicit
/// GPU execution provider (registered first) keeps priority.
fn memory_opts(builder: SessionBuilder) -> Result<SessionBuilder, String> {
    if !docling_core::env::flag("DOCLING_RS_NO_ARENA") {
        return Ok(builder);
    }
    use ort::ep::{cpu::CPU, ExecutionProvider as _};
    let cpu = CPU::default().with_arena_allocator(false);
    let mut builder = builder
        .with_config_entry("session.use_device_allocator_for_initializers", "1")
        .map_err(|e| format!("allocator config: {e}"))?;
    cpu.register(&mut builder)
        .map_err(|e| format!("cpu ep: {e}"))?;
    Ok(builder)
}

/// Create the session for `model_path`, serving ONNX Runtime's *optimized*
/// graph from an on-disk cache when this machine has built it before.
///
/// Session creation is dominated by graph optimization — for the int8 layout
/// model ~0.8 s of the ~0.95 s (measured, 4 threads), for the TableFormer
/// encoder ~1 s — and every process pays it again, per pool worker: on a
/// one-page digital PDF that is more than half of the whole wall time. ONNX
/// Runtime can serialize the optimized graph, and loading that with the
/// optimizer disabled takes ~0.15 s and runs the identical graph (same
/// kernels, same numerics — the corpus output is byte-identical). The cache
/// is keyed on the model file (path, size, mtime), `variant` (session options
/// that change the graph, e.g. a pinned batch dimension), the ONNX Runtime
/// API version and the CPU's SIMD features, because the saved graph carries
/// hardware-specific (NCHWc) kernels; a miss on any of those simply rebuilds.
/// CPU provider only — a GPU EP partitions the graph differently. Location:
/// `DOCLING_RS_GRAPH_CACHE_DIR`, else `$XDG_CACHE_HOME/docling-rs/graphs`,
/// else `~/.cache/docling-rs/graphs`; `DOCLING_RS_NO_GRAPH_CACHE=1` opts out,
/// as does an unwritable directory (the model then loads the ordinary way).
/// Every failure on the cached path falls back to the ordinary load, so a
/// stale or truncated entry costs one rebuild, never a wrong answer.
pub fn commit(builder: SessionBuilder, model_path: &str, variant: &str) -> Result<Session, String> {
    let plain = |mut b: SessionBuilder| b.commit_from_file(model_path).map_err(|e| e.to_string());
    let Some(cache) = graph_cache_path(model_path, variant) else {
        return plain(builder);
    };
    if cache.is_file() {
        let hit = builder
            .clone()
            .with_optimization_level(GraphOptimizationLevel::Disable)
            .map_err(|e| e.to_string())
            .and_then(|mut b| b.commit_from_file(&cache).map_err(|e| e.to_string()));
        match hit {
            Ok(session) => {
                docling_core::debug_log!("docling-onnx: graph cache hit {}", cache.display());
                return Ok(session);
            }
            Err(e) => {
                docling_core::debug_log!(
                    "docling-onnx: graph cache entry {} unusable ({e}); rebuilding",
                    cache.display()
                );
                let _ = std::fs::remove_file(&cache);
            }
        }
    }
    // Miss: build normally, asking ORT to serialize the optimized graph next
    // to the final name, then publish it atomically. Two workers racing here
    // both write their own temp file; whichever renames first wins, the other
    // overwrites with identical content.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = cache.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let built = builder
        .clone()
        .with_optimized_model_path(&tmp)
        .map_err(|e| e.to_string())
        .and_then(plain);
    match built {
        Ok(session) => {
            // ORT serializes the optimized graph as a side effect of session
            // creation and does not fail the session when that write comes up
            // short (a full disk truncates the file and the session still
            // initializes from memory). Publishing such a prefix would poison
            // the cache: every later process would try it, fail, remove it and
            // rebuild — a ~50 s cold start for the TableFormer graphs, seen
            // exactly once after a disk-full episode. So check the protobuf
            // skeleton first: every top-level field's declared length must end
            // within the file (a truncation cuts inside the graph field).
            if !onnx_file_complete(&tmp) {
                docling_core::debug_log!(
                    "docling-onnx: graph cache write of {} incomplete (disk full?); not kept",
                    cache.display()
                );
                let _ = std::fs::remove_file(&tmp);
            } else if std::fs::rename(&tmp, &cache).is_err() {
                let _ = std::fs::remove_file(&tmp);
            } else {
                docling_core::debug_log!("docling-onnx: graph cache wrote {}", cache.display());
            }
            Ok(session)
        }
        Err(_) => {
            // Serialization itself may be what failed (read-only dir, disk
            // full): load without it before giving up.
            let _ = std::fs::remove_file(&tmp);
            plain(builder)
        }
    }
}

/// Whether `path` holds a structurally complete protobuf message: walk the
/// top-level fields of the serialized `ModelProto`, seeking over each
/// length-delimited body, and require the walk to land exactly on EOF. A file
/// cut short by a failed write (disk full) fails this — the truncation lands
/// inside the multi-megabyte `graph` field, whose length prefix then promises
/// more bytes than exist — while costing only a handful of reads, never a
/// parse of the weights. Malformed wire types or zero length fail too.
fn onnx_file_complete(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(len) = f.metadata().map(|m| m.len()) else {
        return false;
    };
    if len == 0 {
        return false;
    }
    let mut pos = 0u64;
    let read_varint = |f: &mut std::fs::File, pos: &mut u64| -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let mut b = [0u8; 1];
            f.read_exact(&mut b).ok()?;
            *pos += 1;
            v |= u64::from(b[0] & 0x7f) << shift;
            if b[0] & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    };
    while pos < len {
        let Some(key) = read_varint(&mut f, &mut pos) else {
            return false;
        };
        let skip = match key & 7 {
            0 => {
                // varint payload
                if read_varint(&mut f, &mut pos).is_none() {
                    return false;
                }
                0
            }
            1 => 8,
            2 => match read_varint(&mut f, &mut pos) {
                Some(n) => n,
                None => return false,
            },
            5 => 4,
            _ => return false,
        };
        if skip > len - pos {
            return false;
        }
        if skip > 0 && f.seek(SeekFrom::Current(skip as i64)).is_err() {
            return false;
        }
        pos += skip;
    }
    pos == len
}

/// Where the optimized graph for `model_path` + `variant` lives on this
/// machine, or `None` when caching is off (env, non-CPU provider, no usable
/// directory, unreadable model file).
fn graph_cache_path(model_path: &str, variant: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    if docling_core::env::flag("DOCLING_RS_NO_GRAPH_CACHE") || choice() != Ep::Cpu {
        return None;
    }
    let dir = graph_cache_dir()?;
    let meta = std::fs::metadata(model_path).ok()?;
    let canonical = std::fs::canonicalize(model_path).unwrap_or_else(|_| PathBuf::from(model_path));
    let mut h = std::hash::DefaultHasher::new();
    canonical.hash(&mut h);
    meta.len().hash(&mut h);
    if let Ok(m) = meta.modified() {
        if let Ok(d) = m.duration_since(std::time::UNIX_EPOCH) {
            d.as_nanos().hash(&mut h);
        }
    }
    variant.hash(&mut h);
    ort::MINOR_VERSION.hash(&mut h);
    cpu_features().hash(&mut h);
    let stem = Path::new(model_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model");
    Some(dir.join(format!("{stem}-{:016x}.onnx", h.finish())))
}

fn graph_cache_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = if let Some(d) = docling_core::env::nonempty("DOCLING_RS_GRAPH_CACHE_DIR") {
            PathBuf::from(d)
        } else if let Some(x) = docling_core::env::nonempty("XDG_CACHE_HOME") {
            PathBuf::from(x).join("docling-rs").join("graphs")
        } else if let Some(home) = docling_core::env::nonempty("HOME") {
            PathBuf::from(home)
                .join(".cache")
                .join("docling-rs")
                .join("graphs")
        } else {
            return None;
        };
        std::fs::create_dir_all(&dir).ok()?;
        // Writable? A read-only cache (a baked container layer) still serves hits.
        Some(dir)
    })
    .clone()
}

/// The SIMD feature set the saved graph was specialized for.
fn cpu_features() -> String {
    let arch = std::env::consts::ARCH;
    // Only x86_64 has runtime-detected feature tiers worth keying the cache
    // on; elsewhere the arch alone is the key (and a `mut` String would be an
    // `unused_mut` warning on the aarch64 macOS CI runner).
    #[cfg(not(target_arch = "x86_64"))]
    {
        String::from(arch)
    }
    #[cfg(target_arch = "x86_64")]
    {
        let mut s = String::from(arch);
        for (name, on) in [
            ("avx", std::arch::is_x86_feature_detected!("avx")),
            ("avx2", std::arch::is_x86_feature_detected!("avx2")),
            ("fma", std::arch::is_x86_feature_detected!("fma")),
            ("avx512f", std::arch::is_x86_feature_detected!("avx512f")),
            ("avx512bw", std::arch::is_x86_feature_detected!("avx512bw")),
            (
                "avx512vnni",
                std::arch::is_x86_feature_detected!("avx512vnni"),
            ),
        ] {
            if on {
                s.push(' ');
                s.push_str(name);
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_names_and_aliases() {
        assert_eq!(parse(""), Some(Ep::Cpu));
        assert_eq!(parse("cpu"), Some(Ep::Cpu));
        assert_eq!(parse("CUDA"), Some(Ep::Cuda));
        assert_eq!(parse(" cuda "), Some(Ep::Cuda));
        assert_eq!(parse("tensorrt"), Some(Ep::TensorRt));
        assert_eq!(parse("trt"), Some(Ep::TensorRt));
        assert_eq!(parse("directml"), Some(Ep::DirectMl));
        assert_eq!(parse("dml"), Some(Ep::DirectMl));
        assert_eq!(parse("CoreML"), Some(Ep::CoreMl));
        assert_eq!(parse("xnnpack"), Some(Ep::Xnnpack));
        assert_eq!(parse("auto"), Some(Ep::Auto));
    }

    #[test]
    fn parse_rejects_unknown() {
        assert_eq!(parse("gpu"), None);
        assert_eq!(parse("rocm"), None);
        assert_eq!(parse("cuda:0"), None);
    }

    #[test]
    fn cpu_and_auto_are_always_compiled() {
        // `choice()` relies on this to keep the unreachable!() arm honest.
        assert!(compiled(Ep::Cpu));
        assert!(compiled(Ep::Auto));
    }

    #[test]
    fn unset_defaults_to_auto_exactly_in_gpu_builds() {
        // CI's ep-features matrix runs this with each GPU feature on, the
        // plain test job with none — both arms get exercised.
        #[cfg(any(
            feature = "cuda",
            feature = "tensorrt",
            feature = "directml",
            feature = "coreml",
            feature = "xnnpack"
        ))]
        assert_eq!(default_choice(), Ep::Auto);
        #[cfg(not(any(
            feature = "cuda",
            feature = "tensorrt",
            feature = "directml",
            feature = "coreml",
            feature = "xnnpack"
        )))]
        assert_eq!(default_choice(), Ep::Cpu);
    }
}

#[cfg(test)]
mod cache_guard_tests {
    use super::onnx_file_complete;

    fn write(bytes: &[u8]) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "docling-onnx-guard-{}-{}.onnx",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    /// A ModelProto-shaped message: `ir_version = 7` (field 1, varint), then a
    /// `graph` (field 7, length-delimited) of 300 bytes — long enough for a
    /// two-byte length prefix like the real thing.
    fn model_like() -> Vec<u8> {
        let mut v = vec![0x08, 0x07, 0x3a, 0xac, 0x02];
        v.extend(std::iter::repeat_n(0x55u8, 300));
        v
    }

    #[test]
    fn complete_file_passes_truncated_fails() {
        let full = model_like();
        assert!(onnx_file_complete(&write(&full)));
        // cut inside the graph body — what a disk-full write leaves behind
        assert!(!onnx_file_complete(&write(&full[..full.len() - 1])));
        assert!(!onnx_file_complete(&write(&full[..40])));
        // cut inside the length prefix itself
        assert!(!onnx_file_complete(&write(&full[..4])));
        assert!(!onnx_file_complete(&write(&[])));
        // trailing garbage that is not a valid field key/wire type
        let mut bad = full.clone();
        bad.push(0x07);
        assert!(!onnx_file_complete(&write(&bad)));
    }

    #[test]
    fn real_optimized_graph_passes() {
        // Any real ONNX file in the repo's model dir, when present.
        for p in [
            "../../.models/tableformer/bbox.onnx",
            "../../.models/layout_heron_int8.onnx",
        ] {
            let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(p);
            if p.is_file() {
                assert!(onnx_file_complete(&p), "{}", p.display());
            }
        }
    }
}
