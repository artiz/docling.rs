//! The converter side of the PII redaction pass (#621): detector assembly
//! (the pattern detector, plus the NER model under the `ner` feature) and
//! the image handling (`BoxOut` through the OCR models) around
//! `docling_core::redact`, which does the walking and replacing.

use docling_core::redact::{CompositeDetector, PatternDetector, PiiDetector, Redactor};
use docling_core::{DoclingDocument, ImageRedaction, RedactionOptions, RedactionReport};

use crate::error::ConversionError;

/// The detectors for `opts`: the pattern detector always, the NER model
/// when a NER kind is in scope and the model loads. A missing model warns
/// once per process and the pass continues pattern-only (the names stay —
/// a documented degradation, not a silent one). An invalid custom regex is
/// the caller's error.
pub(crate) fn detector(opts: &RedactionOptions) -> Result<CompositeDetector, ConversionError> {
    let pattern = PatternDetector::new(opts).map_err(|e| ConversionError::Parse(e.0))?;
    let mut detectors: Vec<Box<dyn PiiDetector + Send + Sync>> = vec![Box::new(pattern)];
    if opts.wants_ner() {
        if let Some(ner) = ner_detector() {
            detectors.push(Box::new(ner));
        }
    }
    Ok(CompositeDetector(detectors))
}

/// The process-wide NER model (#621): loaded on first use, shared by every
/// conversion — the 430 MB BERT graph is not reloaded per document.
#[cfg(feature = "ner")]
fn ner_detector() -> Option<SharedNer> {
    use std::sync::{Arc, OnceLock};
    static SLOT: OnceLock<Option<Arc<docling_pdf::ner::NerDetector>>> = OnceLock::new();
    SLOT.get_or_init(|| match docling_pdf::ner::NerDetector::load() {
        Ok(d) => Some(Arc::new(d)),
        Err(e) => {
            eprintln!(
                "warning: NER model unavailable ({e}); redacting by pattern only — names, \
                 organizations and locations stay. Put dslim/bert-base-NER's ONNX export under \
                 .models/ner/ (model.onnx, tokenizer.json, config.json) or set DOCLING_RS_NER_DIR"
            );
            None
        }
    })
    .clone()
    .map(SharedNer)
}

#[cfg(feature = "ner")]
struct SharedNer(std::sync::Arc<docling_pdf::ner::NerDetector>);

#[cfg(feature = "ner")]
impl PiiDetector for SharedNer {
    fn detect(&self, text: &str) -> Vec<docling_core::redact::Span> {
        self.0.detect(text)
    }
}

/// Without the `ner` feature there is no model to run: one warning, the
/// pattern kinds only.
#[cfg(not(feature = "ner"))]
fn ner_detector() -> Option<NoNer> {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        eprintln!(
            "warning: this build has no NER detector (rebuild with the `ner` feature); \
             redacting by pattern only — names, organizations and locations stay"
        );
    });
    None
}

#[cfg(not(feature = "ner"))]
struct NoNer;

#[cfg(not(feature = "ner"))]
impl PiiDetector for NoNer {
    fn detect(&self, _text: &str) -> Vec<docling_core::redact::Span> {
        Vec::new()
    }
}

#[cfg(feature = "pdf")]
/// `ImageRedaction::BoxOut` over every embedded image and page render:
/// OCR each one with the pipeline's recognizer and paint over the lines the
/// pass flags. Without the OCR models (or the `pdf` feature) the images are
/// dropped instead, with one warning — never kept unread. The repainted
/// images are PNG.
pub(crate) fn box_out_images(
    document: &mut DoclingDocument,
    redactor: &Redactor<'_>,
    ocr: Option<&mut docling_pdf::Pipeline>,
) {
    let Some(pipeline) = ocr else {
        warn_box_out_unavailable("no OCR pipeline");
        drop_images(document);
        return;
    };
    let mut failed: Option<String> = None;
    let mut repaint = |img: &mut docling_core::PictureImage| {
        if failed.is_some() {
            return;
        }
        match pipeline.box_out(&img.data, &|t| redactor.has_pii(t)) {
            Ok(Some(png)) => {
                if let Some((w, h)) = png_size(&png) {
                    img.width = w;
                    img.height = h;
                }
                img.mimetype = "image/png".to_string();
                img.data = png;
            }
            Ok(None) => {}
            Err(e) => failed = Some(e.to_string()),
        }
    };
    document.for_each_picture_mut(&mut |node| {
        if let docling_core::Node::Picture {
            image: Some(img), ..
        } = node
        {
            repaint(img);
        }
    });
    if let Some(tree) = document.tree.as_mut() {
        for item in tree.items.iter_mut() {
            if let docling_core::tree::TreeKind::Picture {
                image: Some(img), ..
            } = &mut item.kind
            {
                repaint(img);
            }
        }
    }
    for img in document.page_images.values_mut() {
        repaint(img);
    }
    if let Some(e) = failed {
        warn_box_out_unavailable(&e);
        drop_images(document);
    }
}

pub(crate) fn warn_box_out_unavailable(why: &str) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        eprintln!(
            "warning: redact_images=box_out needs the OCR models ({why}); dropping the images instead"
        );
    });
}

/// Every embedded image and page render gone (what `ImageRedaction::Drop`
/// does, and what `BoxOut` degrades to).
pub(crate) fn drop_images(document: &mut DoclingDocument) {
    document.for_each_picture_mut(&mut |node| {
        if let docling_core::Node::Picture { image, .. } = node {
            *image = None;
        }
    });
    if let Some(tree) = document.tree.as_mut() {
        for item in tree.items.iter_mut() {
            if let docling_core::tree::TreeKind::Picture { image, .. } = &mut item.kind {
                *image = None;
            }
        }
    }
    document.page_images.clear();
}

/// Pixel size from a PNG IHDR.
#[cfg(feature = "pdf")]
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let be = |i: usize| u32::from_be_bytes([png[i], png[i + 1], png[i + 2], png[i + 3]]);
    Some((be(16), be(20)))
}

/// The whole pass for one buffered conversion: images first (so the OCR
/// lines are judged before the text is rewritten — the detector is
/// stateless, so the order does not change what is found), then the text.
#[cfg(feature = "pdf")]
pub(crate) fn run(
    document: &mut DoclingDocument,
    opts: &RedactionOptions,
    detector: &dyn PiiDetector,
    ocr: Option<&mut docling_pdf::Pipeline>,
) -> RedactionReport {
    let mut redactor = Redactor::new(opts, detector);
    if opts.images == ImageRedaction::BoxOut {
        box_out_images(document, &redactor, ocr);
    }
    redactor.redact_document(document);
    redactor.finish()
}

/// Without the `pdf` feature there is no OCR to box images out with:
/// `BoxOut` drops them, with one warning.
#[cfg(not(feature = "pdf"))]
pub(crate) fn run(
    document: &mut DoclingDocument,
    opts: &RedactionOptions,
    detector: &dyn PiiDetector,
) -> RedactionReport {
    let mut redactor = Redactor::new(opts, detector);
    if opts.images == ImageRedaction::BoxOut {
        warn_box_out_unavailable("built without the `pdf` feature");
        drop_images(document);
    }
    redactor.redact_document(document);
    redactor.finish()
}
