//! A one-page synthetic PDF for the text-layer tests: Helvetica as `/F1`,
//! uncompressed, so a test spells its content stream inline.

/// A one-page A4 PDF drawing `content` in Helvetica (`/F1`, WinAnsi).
pub fn pdf_bytes(content: &[u8]) -> Vec<u8> {
    let stream = [
        format!("<</Length {}>>stream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat();
    let objs: Vec<Vec<u8>> = vec![
        b"<</Type/Catalog/Pages 2 0 R>>".to_vec(),
        b"<</Type/Pages/Kids[3 0 R]/Count 1>>".to_vec(),
        b"<</Type/Page/Parent 2 0 R/MediaBox[0 0 595 842]/Contents 4 0 R\
          /Resources<</Font<</F1 5 0 R>>>>>>"
            .to_vec(),
        stream,
        b"<</Type/Font/Subtype/Type1/BaseFont/Helvetica/Encoding/WinAnsiEncoding>>".to_vec(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"endobj\n");
    }
    let xref_at = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in &offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer<</Size {}/Root 1 0 R>>\nstartxref\n{xref_at}\n%%EOF\n",
            objs.len() + 1
        )
        .as_bytes(),
    );
    out
}
