//! Runtime-asset path resolution shared by every crate.
//!
//! All optional runtime assets (ONNX models, OCR dictionaries, the chunker
//! tokenizer) live under one directory, `.models/` — dot-prefixed like
//! `.pdfium/`, so the plain `models` name stays free for source code.
//! Resolution is CWD-relative with an executable-directory fallback.

/// Resolve a default (CWD-relative) asset path. If it doesn't exist relative
/// to the current directory, a `.models/…` path is tried under
/// `$DOCLING_RS_MODELS_DIR` (#285 — a whole-directory override, so the
/// engine's *own* selection logic — the OCR language pair, the int8
/// preference chains — keeps working against a relocated model set; the
/// Python bindings point it at their per-user cache). After that, try next
/// to the executable and one level above it (following symlinks — the layout
/// `scripts/install/install.sh` produces: `/usr/local/bin/docling-rs` →
/// `/usr/local/docling.rs/bin/docling-rs` with `.models/` and `.pdfium/` in
/// `/usr/local/docling.rs`). Returns `rel` unchanged when nothing exists
/// anywhere, so callers' error messages keep the familiar path. Explicit
/// per-file env overrides never reach this.
pub fn resolve(rel: &str) -> String {
    if std::path::Path::new(rel).exists() {
        return rel.to_string();
    }
    if let Some(stripped) = rel.strip_prefix(".models/") {
        if let Some(dir) = crate::env::nonempty("DOCLING_RS_MODELS_DIR") {
            let p = std::path::Path::new(&dir).join(stripped);
            if p.exists() {
                return p.to_string_lossy().into_owned();
            }
        }
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

    /// `.models/…` resolves under `$DOCLING_RS_MODELS_DIR` when the file
    /// exists there (#285) — and only then; a miss falls through unchanged.
    #[test]
    fn models_dir_override_applies_to_dot_models_paths() {
        let dir = std::env::temp_dir().join(format!("docling-assets-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/x.onnx"), b"x").unwrap();
        std::env::set_var("DOCLING_RS_MODELS_DIR", &dir);
        assert_eq!(
            resolve(".models/sub/x.onnx"),
            dir.join("sub/x.onnx").to_string_lossy()
        );
        // Missing under the override → the input comes back unchanged.
        assert_eq!(resolve(".models/sub/y.onnx"), ".models/sub/y.onnx");
        // Non-.models assets are not redirected.
        assert_eq!(resolve(".pdfium/lib/nope.so"), ".pdfium/lib/nope.so");
        std::env::remove_var("DOCLING_RS_MODELS_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
