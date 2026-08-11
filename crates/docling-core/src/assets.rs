//! Runtime-asset path resolution shared by every crate.
//!
//! All optional runtime assets (ONNX models, OCR dictionaries, the chunker
//! tokenizer) live under one directory, [`MODELS_DIR`] (`.models/`) —
//! dot-prefixed like `.pdfium/`, so the plain `models` name stays free for
//! source code. Resolution is CWD-relative with an executable-directory
//! fallback.

/// The runtime-asset directory, relative to the working directory.
pub const MODELS_DIR: &str = ".models";

/// Resolve a default (CWD-relative) asset path. If it doesn't exist relative
/// to the current directory, try next to the executable and one level above
/// it (following symlinks — the layout `scripts/install/install.sh` produces:
/// `/usr/local/bin/docling-rs` → `/usr/local/docling.rs/bin/docling-rs` with
/// `.models/` and `.pdfium/` in `/usr/local/docling.rs`). Returns `rel`
/// unchanged when nothing exists anywhere, so callers' error messages keep
/// the familiar path. Explicit env overrides never reach this.
pub fn resolve(rel: &str) -> String {
    if std::path::Path::new(rel).exists() {
        return rel.to_string();
    }
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf));
    if let Some(dir) = dir {
        for base in [Some(dir.as_path()), dir.parent()].into_iter().flatten() {
            let p = base.join(rel);
            if p.exists() {
                return p.to_string_lossy().into_owned();
            }
        }
    }
    rel.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_everywhere_returns_input() {
        assert_eq!(
            resolve(".models/definitely/not/there.onnx"),
            ".models/definitely/not/there.onnx"
        );
    }
}
