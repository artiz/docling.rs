//! #916: `render_pages` reports what a render left out instead of drawing a
//! silent placeholder, and names the cap that refused a page.
#![cfg(feature = "ml")]

use docling_pdf::{render_pages, PdfError};

/// A 600x300 pt page drawing one 100x50 `/DCTDecode` image whose stream is
/// not a JPEG; `xref` is omitted (lopdf rebuilds it).
fn planted_image_pdf() -> Vec<u8> {
    let mut pdf = String::from("%PDF-1.4\n");
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 300] /Contents 4 0 R \
         /Resources << /XObject << /Im1 5 0 R >> >> >>"
            .to_string(),
        "<< /Length 32 >>\nstream\nq 100 0 0 50 30 100 cm /Im1 Do Q\nendstream".to_string(),
        "<< /Type /XObject /Subtype /Image /Width 100 /Height 50 /BitsPerComponent 8 \
         /ColorSpace /DeviceRGB /Filter /DCTDecode /Length 25 >>\nstream\n\
         undecodable planted image\nendstream"
            .to_string(),
    ];
    for (i, o) in objs.iter().enumerate() {
        pdf.push_str(&format!("{} 0 obj\n{o}\nendobj\n", i + 1));
    }
    pdf.push_str("trailer\n<< /Size 6 /Root 1 0 R >>\n%%EOF\n");
    pdf.into_bytes()
}

#[test]
fn undecodable_image_is_reported() {
    let pages = render_pages(&planted_image_pdf(), None, None, 1.0).expect("render");
    assert_eq!(pages.len(), 1);
    let om = &pages[0].omissions;
    assert_eq!(om.len(), 1, "{om:?}");
    assert_eq!(om[0].object, Some((5, 0)));
    assert!(
        om[0].reason.contains("JPEG not decoded"),
        "{}",
        om[0].reason
    );
    // Drawn at (30, 100) 100x50 pt on a 300 pt page: device y from the top.
    let r = om[0].rect;
    assert!(
        (r[0] - 30.0).abs() < 1.0 && (r[2] - 130.0).abs() < 1.0,
        "{r:?}"
    );
    assert!(
        (r[1] - 150.0).abs() < 1.0 && (r[3] - 200.0).abs() < 1.0,
        "{r:?}"
    );
}

#[test]
fn clean_page_has_no_omissions() {
    let pdf = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] >>\nendobj\ntrailer\n<< /Size 4 /Root 1 0 R >>\n%%EOF\n";
    let pages = render_pages(pdf, None, None, 1.0).expect("render");
    assert!(pages[0].omissions.is_empty());
}

#[test]
fn refused_page_names_the_cap() {
    // 600 pt at scale 30 = 18000 px > the 15000 px default per-side cap.
    let err = render_pages(&planted_image_pdf(), None, None, 30.0).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("per-side cap"), "{msg}");
    assert!(msg.contains("page 1"), "{msg}");
    assert!(!matches!(err, PdfError::Document(ref m) if m.contains("no page renderer")));
}
