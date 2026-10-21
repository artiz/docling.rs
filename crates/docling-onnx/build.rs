//! Apple targets: link clang's compiler-rt builtins next to pyke's static
//! ONNX Runtime (#816).
//!
//! pyke ships `aarch64-apple-darwin` only as a CoreML-enabled build (ort-sys's
//! `dist.tsv` has no plain CPU variant for it), and the CoreML provider's
//! Objective-C objects (`host_utils.mm`, `model.mm`) carry `@available`
//! checks, which clang lowers to calls of `___isPlatformVersionAtLeast` — a
//! compiler-rt builtin, not a libSystem function. clang links
//! `libclang_rt.osx.a` itself when it drives the link; rustc passes
//! `-nodefaultlibs`, so nothing provides the symbol. std exports a weak copy
//! since Rust 1.91 (rust-lang/rust#138944, weak in #146343) — which is why a
//! dev build links — but thin LTO, this workspace's and docling-py's release
//! profile, drops the unreferenced weak definition before the archive asks
//! for it, and the release link ends with `Undefined symbols for architecture
//! arm64: "___isPlatformVersionAtLeast"` (the first macOS cli-binaries run).
//! Linking the builtins archive the way clang would is the fix every
//! Rust + Objective-C link ends up with.
//!
//! `static:-bundle`: the archive is recorded for the final link instead of
//! copied into the rlib, and — unlike `cargo:rustc-link-arg`, which never
//! leaves the package that emitted it — it reaches every dependent: the CLI,
//! the FFI cdylib, the Node and Python extension modules. The linker pulls
//! only the members something references, so the few builtins are all a
//! binary gains.
//!
//! Skipped for the `load-dynamic` build (no static ONNX Runtime is linked)
//! and for every non-Apple target. A toolchain without the archive gets a
//! build warning that names it; the link then fails the way it did, with
//! the warning next to it.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_VENDOR").as_deref() != Ok("apple")
        || env::var_os("CARGO_FEATURE_LOAD_DYNAMIC").is_some()
    {
        return;
    }
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let sim = env::var("CARGO_CFG_TARGET_ABI").as_deref() == Ok("sim");
    // compiler-rt names the builtins archive per Apple platform
    // (`lib/darwin/libclang_rt.<platform>.a`); pyke ships ONNX Runtime for
    // macOS, iOS and the iOS simulator.
    let platform = match (os.as_str(), sim) {
        ("macos", _) => "osx",
        ("ios", false) => "ios",
        ("ios", true) => "iossim",
        _ => {
            println!("cargo:warning=docling-onnx: no compiler-rt builtins mapping for target_os={os}; the ONNX Runtime link may miss ___isPlatformVersionAtLeast");
            return;
        }
    };
    let Some(dir) = builtins_dir() else {
        println!(
            "cargo:warning=docling-onnx: clang's compiler-rt builtins (libclang_rt.{platform}.a) \
             not found via `cc --print-resource-dir` / `xcrun clang`; the static ONNX Runtime \
             link may fail on ___isPlatformVersionAtLeast — install the Xcode Command Line \
             Tools or set CC to a clang"
        );
        return;
    };
    let archive = dir.join(format!("libclang_rt.{platform}.a"));
    if !archive.is_file() {
        println!(
            "cargo:warning=docling-onnx: {} is missing; the static ONNX Runtime link may fail on \
             ___isPlatformVersionAtLeast",
            archive.display()
        );
        return;
    }
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=static:-bundle=clang_rt.{platform}");
}

/// `<clang resource dir>/lib/darwin` of the C compiler cargo's `cc` crate
/// would pick for this target (`CC_<target>`, `TARGET_CC`, `CC`, else `cc`),
/// falling back to the Xcode toolchain's clang through `xcrun`.
fn builtins_dir() -> Option<PathBuf> {
    let target = env::var("TARGET").unwrap_or_default();
    let compilers = [
        format!("CC_{target}"),
        format!("CC_{}", target.replace('-', "_")),
        "TARGET_CC".to_string(),
        "CC".to_string(),
    ];
    for var in &compilers {
        println!("cargo:rerun-if-env-changed={var}");
    }
    let mut candidates: Vec<Vec<String>> = compilers
        .iter()
        .filter_map(|var| env::var(var).ok())
        .filter(|cc| !cc.is_empty())
        .map(|cc| vec![cc])
        .collect();
    candidates.push(vec!["cc".to_string()]);
    candidates.push(vec!["xcrun".to_string(), "clang".to_string()]);
    candidates.into_iter().find_map(|argv| {
        let out = Command::new(&argv[0])
            .args(&argv[1..])
            .arg("--print-resource-dir")
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let resource = String::from_utf8(out.stdout).ok()?;
        let dir = Path::new(resource.trim()).join("lib").join("darwin");
        dir.is_dir().then_some(dir)
    })
}
