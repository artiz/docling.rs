//! #917: the `render` feature exposes the pure-Rust page renderer without the
//! inference stack — `ort` / `docling-onnx` must stay behind `ml` alone.

/// The manifest's `name = [ ... ]` feature block, as text.
fn feature_block(manifest: &str, name: &str) -> String {
    let start = manifest
        .find(&format!("\n{name} = ["))
        .unwrap_or_else(|| panic!("feature {name}"));
    let rest = &manifest[start..];
    rest[..rest.find("]\n").unwrap()].to_string()
}

#[test]
fn render_feature_does_not_pull_ort() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("Cargo.toml");
    let render = feature_block(&manifest, "render");
    for forbidden in ["dep:ort", "dep:docling-onnx", "dep:tokenizers", "\"ml\""] {
        assert!(
            !render.contains(forbidden),
            "`render` must not enable {forbidden}"
        );
    }
    assert!(
        feature_block(&manifest, "ml").contains("\"render\""),
        "`ml` implies `render`"
    );
}

#[cfg(feature = "render")]
#[test]
fn render_pages_works_without_ml() {
    let pdf = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] >>\nendobj\ntrailer\n<< /Size 4 /Root 1 0 R >>\n%%EOF\n";
    let pages = docling_pdf::render_pages(pdf, None, None, 1.0).expect("render");
    assert_eq!(pages.len(), 1);
    assert_eq!((pages[0].width, pages[0].height), (100, 50));
}
