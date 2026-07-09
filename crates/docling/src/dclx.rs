//! `.dclx` packaging: the DocLang OPC archive (`doclang.pack` counterpart).
//!
//! Layout (fixed): `[Content_Types].xml`, `_rels/.rels` (both static bytes,
//! matching the Python `doclang` package verbatim), and `document.xml` — the
//! [`DoclingDocument::export_to_doclang`] markup plus a single trailing
//! newline. Entries are deflate-compressed and written in the reference's
//! lexicographic order. Picture/page image parts are not emitted (our default
//! export matches docling's placeholder image mode, which stores no images).

use std::io::Write;

use docling_core::DoclingDocument;
use zip::write::SimpleFileOptions;

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="png" ContentType="image/png"/>
  <Default Extension="jpg" ContentType="image/jpeg"/>
  <Default Extension="jpeg" ContentType="image/jpeg"/>
  <Default Extension="webp" ContentType="image/webp"/>
  <Override PartName="/document.xml" ContentType="application/vnd.doclang.document+xml"/>
</Types>
"#;

const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1"
    Type="http://doclang.ai/ns/package/2026/relationships/document"
    Target="document.xml"/>
</Relationships>
"#;

/// Serialize `doc` into `.dclx` bytes.
pub fn to_dclx_bytes(doc: &DoclingDocument) -> Vec<u8> {
    let xml = format!("{}\n", doc.export_to_doclang());
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        // Reference order: lexicographic over the staged tree.
        zip.start_file("[Content_Types].xml", opts)
            .expect("zip start");
        zip.write_all(CONTENT_TYPES.as_bytes()).expect("zip write");
        zip.start_file("_rels/.rels", opts).expect("zip start");
        zip.write_all(RELS.as_bytes()).expect("zip write");
        zip.start_file("document.xml", opts).expect("zip start");
        zip.write_all(xml.as_bytes()).expect("zip write");
        zip.finish().expect("zip finish");
    }
    buf.into_inner()
}

/// Write `doc` as a `.dclx` archive at `path`.
pub fn save_as_dclx(doc: &DoclingDocument, path: &std::path::Path) -> std::io::Result<()> {
    std::fs::write(path, to_dclx_bytes(doc))
}
