//! Browser/edge (wasm32) bindings for docling.rs's **declarative** converters
//! (issue #79): DOCX, HTML, Markdown, XLSX, PPTX, CSV, AsciiDoc, EPUB, ODF,
//! WebVTT, Email, MHTML, JATS, USPTO, XBRL, LaTeX, JSON, DocLang → Markdown or
//! docling JSON — fully client-side, no server round-trip.
//!
//! Built on `docling` with `default-features = false`: the PDF/image/ASR ML
//! pipelines (pdfium + ONNX Runtime) and the HTTP image fetcher are compiled
//! out — those formats are rejected at convert time with a clear message.
//!
//! ```js
//! import init, { convert } from "./pkg/docling_wasm.js";
//! await init();
//! const md = convert(new Uint8Array(await file.arrayBuffer()), file.name, "md");
//! ```

use docling::{DocumentConverter, InputFormat, SourceDocument};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
fn start() {
    // Panics surface as readable messages in the browser console instead of
    // an opaque `unreachable executed`.
    console_error_panic_hook::set_once();
}

/// The whole conversion body, host-testable (`JsError` can only be
/// constructed on the wasm target, so the JS boundary stays a thin shim).
fn convert_impl(bytes: &[u8], filename: &str, to: Option<&str>) -> Result<String, String> {
    let ext = filename.rsplit('.').next().unwrap_or_default();
    let format = InputFormat::from_extension(ext)
        .ok_or_else(|| format!("unknown or unsupported extension: {filename:?}"))?;
    let source = SourceDocument::from_bytes(filename.to_string(), format, bytes.to_vec());
    let result = DocumentConverter::new()
        .convert(source)
        .map_err(|e| e.to_string())?;
    match to.unwrap_or("md") {
        "md" | "markdown" => Ok(result.document.export_to_markdown()),
        "json" => Ok(result.document.export_to_json()),
        "doclang" => Ok(result.document.export_to_doclang()),
        other => Err(format!(
            "unknown output format {other:?} (expected \"md\", \"json\" or \"doclang\")"
        )),
    }
}

/// Convert a document (as bytes + filename, the extension drives format
/// detection) to `to`: `"md"` (Markdown, default), `"json"` (docling-core's
/// `DoclingDocument` wire format, schema 1.10.0) or `"doclang"` (docling's
/// DocLang XML serialization).
#[wasm_bindgen]
pub fn convert(bytes: &[u8], filename: &str, to: Option<String>) -> Result<String, JsError> {
    convert_impl(bytes, filename, to.as_deref()).map_err(|e| JsError::new(&e))
}

/// The file extensions this build can convert, as a JSON string array —
/// handy for an `<input accept=…>` filter. The ML formats (pdf, images,
/// audio, METS) are excluded: they are not compiled into the wasm build.
#[wasm_bindgen]
pub fn supported_extensions() -> String {
    // Keep in sync with `InputFormat::from_extension` minus the ML formats
    // (pdf, images, audio, mets tarballs).
    let exts = [
        "docx", "dotx", "docm", "dotm", "pptx", "potx", "ppsx", "pptm", "potm", "ppsm", "md",
        "txt", "text", "qmd", "rmd", "html", "htm", "xhtml", "xml", "nxml", "dclg", "dclx", "adoc",
        "asciidoc", "asc", "csv", "xlsx", "xlsm", "odt", "ott", "ods", "ots", "odp", "otp", "json",
        "vtt", "tex", "latex", "eml", "epub", "mhtml", "mht",
    ];
    serde_json::to_string(exts.as_slice()).expect("static array serializes")
}

/// The docling.rs version this module was built from.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    // Host-side (`cargo test -p docling-wasm`) sanity of the conversion body —
    // the wasm-bindgen layer is exercised by the browser demo.
    use super::*;

    #[test]
    fn markdown_roundtrip() {
        let md = b"# Title\n\nHello *world*\n";
        let out = convert_impl(md, "note.md", None).unwrap();
        assert!(out.contains("# Title"));
        let json = convert_impl(md, "note.md", Some("json")).unwrap();
        assert!(json.contains("\"schema_name\""));
    }

    #[test]
    fn ml_formats_rejected() {
        let err = convert_impl(b"%PDF-1.4", "doc.pdf", None).unwrap_err();
        assert!(
            err.contains("pdf"),
            "should point at the missing feature: {err}"
        );
    }

    #[test]
    fn docx_converts() {
        // A real corpus DOCX through the wasm entry path on the host.
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docling/tests/data/docx/sources/docx_lists.docx"
        ))
        .expect("corpus docx");
        let out = convert_impl(&bytes, "docx_lists.docx", None).unwrap();
        assert!(!out.trim().is_empty());
    }

    #[test]
    fn extensions_json_parses() {
        let v: Vec<String> = serde_json::from_str(&supported_extensions()).unwrap();
        assert!(v.contains(&"docx".to_string()));
        assert!(!v.contains(&"pdf".to_string()));
    }
}
