//! Browser/edge (wasm32) bindings for docling.rs's **declarative** converters
//! (issue #79): DOCX, HTML, Markdown, XLSX, PPTX, CSV, AsciiDoc, EPUB, ODF,
//! RTF, WebVTT, Email, MHTML, JATS, USPTO, XBRL, LaTeX, JSON, DocLang →
//! Markdown or docling JSON — fully client-side, no server round-trip.
//!
//! Built on `docling` with `default-features = false` plus `pdf-text`: a PDF's
//! **embedded text layer** converts too (pure-Rust parser, same extraction as
//! the native `--no-ocr` flag — flat paragraphs, no headings/tables/pictures).
//! The ML pipelines (pdfium + ONNX Runtime) and the HTTP image fetcher are
//! compiled out — scanned PDFs, images, and audio are rejected at convert time
//! with a clear message.
//!
//! ```js
//! import init, { convert } from "./pkg/docling_wasm.js";
//! await init();
//! const md = convert(new Uint8Array(await file.arrayBuffer()), file.name, "md");
//! ```

use docling::{DocumentConverter, ImageMode, InputFormat, SourceDocument};
use wasm_bindgen::prelude::*;

#[cfg(feature = "ocr")]
mod digital;
#[cfg(feature = "ocr")]
mod ocr;
#[cfg(feature = "ocr")]
mod scanned;
#[cfg(feature = "ocr")]
mod tableformer;
#[cfg(feature = "ocr")]
pub use digital::DigitalConverter;
#[cfg(feature = "ocr")]
pub use ocr::ocr_image;
#[cfg(feature = "ocr")]
pub use scanned::{convert_scanned_image, ScannedConverter};

#[wasm_bindgen(start)]
fn start() {
    // Panics surface as readable messages in the browser console instead of
    // an opaque `unreachable executed`.
    console_error_panic_hook::set_once();
}

/// The whole conversion body, host-testable (`JsError` can only be
/// constructed on the wasm target, so the JS boundary stays a thin shim).
fn convert_impl(
    bytes: &[u8],
    filename: &str,
    to: Option<&str>,
    images: Option<&str>,
    max_pages: Option<u32>,
) -> Result<String, String> {
    let ext = filename.rsplit('.').next().unwrap_or_default();
    let format = InputFormat::from_extension(ext)
        .ok_or_else(|| format!("unknown or unsupported extension: {filename:?}"))?;
    let source = SourceDocument::from_bytes(filename.to_string(), format, bytes.to_vec());
    let mut converter = DocumentConverter::new();
    // "First N pages" (issue #80's window with first pinned to 1): only PDFs
    // consume it; other formats convert whole, same as the CLI.
    if let Some(n) = max_pages.filter(|&n| n > 0) {
        converter = converter.page_range(1, n as usize);
    }
    let result = converter.convert(source).map_err(|e| e.to_string())?;
    let image_mode = image_mode(images)?;
    match to.unwrap_or("md") {
        // `Referenced` is deliberately unreachable here: it hands the caller
        // loose image files to write next to the Markdown, which a page with no
        // filesystem cannot do — the browser equivalent is `embedded`.
        "md" | "markdown" => Ok(result
            .document
            .export_to_markdown_with_images(image_mode, "artifacts")
            .0),
        "json" => Ok(result.document.export_to_json()),
        "doclang" => Ok(result.document.export_to_doclang()),
        other => Err(format!(
            "unknown output format {other:?} (expected \"md\", \"json\" or \"doclang\")"
        )),
    }
}

/// Picture rendering for Markdown output, mirroring docling-serve's `images`
/// option: `placeholder` (docling's default `<!-- image -->`) or `embedded`
/// (`![Image](data:…;base64,…)`, self-contained — the only way to carry pixels
/// out of a page that cannot write files).
fn image_mode(images: Option<&str>) -> Result<ImageMode, String> {
    match images.unwrap_or("placeholder") {
        "placeholder" => Ok(ImageMode::Placeholder),
        "embedded" => Ok(ImageMode::Embedded),
        other => Err(format!(
            "unknown images={other:?} (expected \"placeholder\" or \"embedded\")"
        )),
    }
}

/// Convert a document (as bytes + filename, the extension drives format
/// detection) to `to`: `"md"` (Markdown, default), `"json"` (docling-core's
/// `DoclingDocument` wire format, schema 1.10.0) or `"doclang"` (docling's
/// DocLang XML serialization).
///
/// `images` controls how pictures render in Markdown — `"placeholder"`
/// (default) or `"embedded"` (base64 data URIs), the same option
/// docling-serve exposes. `max_pages` converts only a PDF's first N pages
/// (issue #80's window, first pinned to 1); other formats ignore it.
#[wasm_bindgen]
pub fn convert(
    bytes: &[u8],
    filename: &str,
    to: Option<String>,
    images: Option<String>,
    max_pages: Option<u32>,
) -> Result<String, JsError> {
    convert_impl(bytes, filename, to.as_deref(), images.as_deref(), max_pages)
        .map_err(|e| JsError::new(&e))
}

/// The file extensions this build can convert, as a JSON string array —
/// handy for an `<input accept=…>` filter. PDF converts via its embedded
/// text layer (`pdf-text`); the remaining ML formats (images, audio, METS)
/// are excluded: they are not compiled into the wasm build.
#[wasm_bindgen]
pub fn supported_extensions() -> String {
    // Keep in sync with `InputFormat::from_extension` minus the ML-only
    // formats (images, audio, video, mets tarballs).
    let exts = [
        "docx", "dotx", "docm", "dotm", "pptx", "potx", "ppsx", "pptm", "potm", "ppsm", "md",
        "txt", "text", "qmd", "rmd", "html", "htm", "xhtml", "xml", "nxml", "dclg", "dclx", "adoc",
        "asciidoc", "asc", "csv", "tsv", "xlsx", "xlsm", "xlsb", "odt", "ott", "ods", "ots", "odp",
        "otp", "sxw", "stw", "sxg", "sxc", "stc", "sxi", "sti", "fodt", "fods", "fodp", "json",
        "sdw", "sda", "sdd", "vor", "abw", "zabw", "awt", "dbf", "dif", "slk", "sylk", "wk1",
        "wk2", "wk3", "wk4", "wks", "wrk", "123", "vtt", "tex", "latex", "eml", "epub", "mhtml",
        "mht", "rtf", "vsdx", "vsdm", "pdf",
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
        let out = convert_impl(md, "note.md", None, None, None).unwrap();
        assert!(out.contains("# Title"));
        let json = convert_impl(md, "note.md", Some("json"), None, None).unwrap();
        assert!(json.contains("\"schema_name\""));
    }

    #[test]
    fn ml_formats_rejected() {
        // Images still need the full ML pipeline.
        let err =
            convert_impl(&[0x89, b'P', b'N', b'G'], "scan.png", None, None, None).unwrap_err();
        assert!(
            err.contains("unknown or unsupported") || err.contains("pdf"),
            "should reject the ML-only format: {err}"
        );
    }

    #[test]
    fn pdf_text_layer_converts() {
        // A text-layer PDF converts via the pure-Rust `pdf-text` path (the
        // exact `--no-ocr` extraction: flat paragraphs in reading order).
        // Under `cargo test --workspace`, feature unification swaps in the
        // full ML pipeline (which needs pdfium + models) — this test is about
        // the text-layer arm, so it only runs in the real wasm feature set.
        if docling::PDF_ML_COMPILED {
            return;
        }
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/data/pdf/sources/code_and_formula.pdf"
        ))
        .expect("corpus pdf");
        let out = convert_impl(&bytes, "code_and_formula.pdf", None, None, None).unwrap();
        assert!(!out.trim().is_empty(), "text layer should extract");
    }

    #[test]
    fn scanned_pdf_reports_missing_text_layer() {
        // A PDF with no embedded text (here: a stub with no content stream)
        // should explain that OCR needs a native build, not return "".
        // Text-layer-arm-only, same as `pdf_text_layer_converts`.
        if docling::PDF_ML_COMPILED {
            return;
        }
        let err = convert_impl(b"%PDF-1.4\n%%EOF", "scan.pdf", None, None, None).unwrap_err();
        assert!(
            err.contains("text layer") || err.contains("OCR"),
            "should point at the missing text layer: {err}"
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
        let out = convert_impl(&bytes, "docx_lists.docx", None, None, None).unwrap();
        assert!(!out.trim().is_empty());
    }

    /// `images=embedded` inlines picture bytes as data URIs (docling-serve's
    /// option, and the only way a page with no filesystem can carry pixels
    /// out); the default stays docling's `<!-- image -->` placeholder.
    #[test]
    fn embedded_images_inline_as_data_uris() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docling/tests/data/docx/sources/word_image_anchors.docx"
        ))
        .expect("corpus docx with images");
        let placeholder =
            convert_impl(&bytes, "word_image_anchors.docx", None, None, None).unwrap();
        assert!(placeholder.contains("<!-- image -->"), "{placeholder}");
        let embedded = convert_impl(
            &bytes,
            "word_image_anchors.docx",
            None,
            Some("embedded"),
            None,
        )
        .unwrap();
        assert!(embedded.contains("](data:image/"), "expected a data URI");
        let err =
            convert_impl(&bytes, "word_image_anchors.docx", None, Some("nope"), None).unwrap_err();
        assert!(err.contains("unknown images="), "{err}");
    }

    #[test]
    fn extensions_json_parses() {
        let v: Vec<String> = serde_json::from_str(&supported_extensions()).unwrap();
        assert!(v.contains(&"docx".to_string()));
        assert!(v.contains(&"pdf".to_string()), "pdf-text is compiled in");
        assert!(!v.contains(&"png".to_string()));
    }
}
