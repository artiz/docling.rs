//! Runtime-asset path resolution shared by every crate.
//!
//! All optional runtime assets (ONNX models, OCR dictionaries, the chunker
//! tokenizer) live under one directory, [`MODELS_DIR`] (`.models/`) —
//! dot-prefixed like `.pdfium/`, so the plain `models` name stays free for
//! source code. Resolution is CWD-relative with an executable-directory
//! fallback, and every lookup transparently falls back to the pre-rename
//! `models/` location so existing installs and checkouts keep working
//! without re-downloading.

/// The default runtime-asset directory, relative to the working directory.
pub const MODELS_DIR: &str = ".models";

/// The pre-rename asset directory, still honored as a read fallback.
pub const LEGACY_MODELS_DIR: &str = "models";

/// Resolve a default (CWD-relative) asset path. If it doesn't exist relative
/// to the current directory, try next to the executable and one level above
/// it (following symlinks — the layout `scripts/install/install.sh` produces:
/// `/usr/local/bin/docling-rs` → `/usr/local/docling.rs/bin/docling-rs` with
/// `.models/` and `.pdfium/` in `/usr/local/docling.rs`). A path under
/// [`MODELS_DIR`] that exists nowhere is retried under
/// [`LEGACY_MODELS_DIR`] through the same chain. Returns `rel` unchanged
/// when nothing exists anywhere, so callers' error messages keep the
/// familiar (new-style) path. Explicit env overrides never reach this.
pub fn resolve(rel: &str) -> String {
    if let Some(found) = try_bases(rel) {
        return found;
    }
    if let Some(legacy) = legacy_of(rel) {
        if let Some(found) = try_bases(&legacy) {
            return found;
        }
    }
    rel.to_string()
}

/// `rel` rewritten from the [`MODELS_DIR`] prefix to [`LEGACY_MODELS_DIR`],
/// when it carries one.
fn legacy_of(rel: &str) -> Option<String> {
    rel.strip_prefix(MODELS_DIR)
        .filter(|rest| rest.is_empty() || rest.starts_with('/'))
        .map(|rest| format!("{LEGACY_MODELS_DIR}{rest}"))
}

/// First existing location for `rel`: the working directory, the
/// executable's directory, its parent.
fn try_bases(rel: &str) -> Option<String> {
    if std::path::Path::new(rel).exists() {
        return Some(rel.to_string());
    }
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))?;
    for base in [Some(dir.as_path()), dir.parent()].into_iter().flatten() {
        let p = base.join(rel);
        if p.exists() {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_rewrite_only_under_models_dir() {
        assert_eq!(
            legacy_of(".models/asr/x.onnx").as_deref(),
            Some("models/asr/x.onnx")
        );
        assert_eq!(legacy_of(".models").as_deref(), Some("models"));
        assert_eq!(legacy_of(".pdfium/lib"), None);
        assert_eq!(legacy_of(".models-extra/x"), None);
    }

    #[test]
    fn missing_everywhere_returns_input() {
        assert_eq!(
            resolve(".models/definitely/not/there.onnx"),
            ".models/definitely/not/there.onnx"
        );
    }
}
