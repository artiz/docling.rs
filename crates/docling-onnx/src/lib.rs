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

use std::sync::OnceLock;

use ort::ep::ExecutionProviderDispatch;
use ort::session::builder::SessionBuilder;

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
