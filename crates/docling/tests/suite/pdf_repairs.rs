//! `pdf_report_repairs` (#934): a PDF conversion reports what the reader
//! repaired to read the input — off by default, and the document is the
//! same either way. Text-layer path, so no models are needed.

use docling::{DocumentConverter, InputFormat, PdfRepair, SourceDocument, TextLayerOnly};

const CONTENT: &[u8] = b"BT /F1 12 Tf 20 100 Td (Repaired stream text) Tj ET\n";

/// A one-page PDF whose content stream declares `extra` bytes more than
/// it holds.
fn pdf(extra: usize) -> Vec<u8> {
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R \
           /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        [
            format!("<< /Length {} >>\nstream\n", CONTENT.len() + extra).as_bytes(),
            CONTENT,
            b"endstream",
        ]
        .concat(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let size = objects.len() + 1;
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

fn convert(converter: DocumentConverter, bytes: Vec<u8>) -> docling::ConversionResult {
    converter
        .text_layer_only(TextLayerOnly::On)
        .convert(SourceDocument::from_bytes("t.pdf", InputFormat::Pdf, bytes))
        .expect("conversion")
}

#[test]
fn repairs_are_reported_only_when_asked() {
    let quiet = convert(DocumentConverter::new().pdf_report_repairs(false), pdf(7));
    assert_eq!(quiet.pdf_repairs, None);

    let asked = convert(DocumentConverter::new().pdf_report_repairs(true), pdf(7));
    let offset = pdf(7).windows(7).position(|w| w == b"4 0 obj").unwrap();
    assert_eq!(
        asked.pdf_repairs,
        Some(vec![PdfRepair::StreamLength {
            object: (4, 0),
            offset,
            declared: Some(CONTENT.len() as i64 + 7),
            recovered: Some(CONTENT.len() - 1),
        }])
    );
    // The document is the same either way.
    assert_eq!(
        asked.document.export_to_markdown(),
        quiet.document.export_to_markdown()
    );
    assert!(asked
        .document
        .export_to_markdown()
        .contains("Repaired stream text"));

    // A well-formed file: asked, and nothing to report.
    let clean = convert(DocumentConverter::new().pdf_report_repairs(true), pdf(0));
    assert_eq!(clean.pdf_repairs, Some(Vec::new()));
}
