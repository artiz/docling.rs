//! #666: RapidOCR's text-line orientation handling on the picture OCR —
//! oriented crops, the 90° turn of tall lines, the 0/180 classifier — and
//! the picture-only OCR scale. Fixtures (`tests/fixtures/textline/`,
//! `generate.py`): an upright line over an upside-down one, a chart with a
//! bottom-to-top axis title, a line and its ~10° skewed twin. Off (the
//! default) each loses the turned line; on, each reads it. Needs the OCR
//! models plus `.models/ocr_textline_ori.onnx`, skipping cleanly without
//! them like the other ML tests.

use std::path::{Path, PathBuf};

use docling::{DocumentConverter, Pipeline, SourceDocument};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The detector, a recognizer pair and the orientation classifier are
/// installed; CWD moves to the repo root, where `.models/` is.
fn models_ready() -> bool {
    let models = repo_root().join(".models");
    let v6 = models.join("ocr_rec_v6.onnx").exists() && models.join("ocr_rec_v6_dict.txt").exists();
    let en = models.join("ocr_rec_en.onnx").exists() && models.join("en_dict.txt").exists();
    (v6 || en)
        && models.join("ocr_det.onnx").exists()
        && models.join("ocr_textline_ori.onnx").exists()
        && std::env::set_current_dir(repo_root()).is_ok()
}

/// The picture OCR's text for a fixture, orientation handling on or off.
fn read(fixture: &str, orient: bool) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/textline")
        .join(fixture);
    let bytes = std::fs::read(path).expect("fixture");
    let mut p = Pipeline::new()
        .expect("pipeline")
        .no_ocr(true)
        .no_table_former(true)
        .ocr_textline_orientation(orient);
    let img = p.decode_picture(&bytes).expect("decode");
    p.ocr_picture(&img)
        .expect("ocr")
        .map(|t| t.text)
        .unwrap_or_default()
}

const UPRIGHT: &str = "Quarterly revenue grew by 12 percent";
const TURNED: &str = "Open the Settings dialog to continue";

/// The classifier turns the upside-down line; without it the recognizer
/// reads it as noise.
#[test]
fn an_upside_down_line_reads_upright() {
    if !models_ready() {
        eprintln!("skipping: OCR models not found");
        return;
    }
    assert_eq!(
        read("upside_down.png", true),
        format!("{UPRIGHT}\n{TURNED}")
    );
    let off = read("upside_down.png", false);
    assert!(off.starts_with(UPRIGHT), "{off:?}");
    assert!(!off.contains(TURNED), "{off:?}");
}

/// The bottom-to-top title is a tall crop: turned 90°, then 180° by the
/// classifier, it reads — off, it is lost.
#[test]
fn a_vertical_axis_title_is_read() {
    if !models_ready() {
        eprintln!("skipping: OCR models not found");
        return;
    }
    assert_eq!(
        read("axis_title.png", true),
        "Revenue in millions\nJan Feb Mar Apr May"
    );
    assert_eq!(read("axis_title.png", false), "Jan Feb Mar Apr May");
}

/// The skewed twin, warped upright out of its rotated rectangle, reads as
/// its upright original; its axis-aligned box alone loses it.
#[test]
fn a_skewed_line_reads_like_its_upright_twin() {
    if !models_ready() {
        eprintln!("skipping: OCR models not found");
        return;
    }
    let line = "Shipping address confirmed";
    assert_eq!(read("skewed.png", true), format!("{line}\n{line}"));
    assert_eq!(read("skewed.png", false), line);
}

/// End to end through the converter: `picture_ocr_scale(1.0)` and the
/// orientation switch on the #645 DOCX read its screenshot's two lines as
/// the default does.
#[test]
fn converter_options_reach_the_picture_ocr() {
    if !models_ready() {
        eprintln!("skipping: OCR models not found");
        return;
    }
    let src = || {
        SourceDocument::from_file(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/docx/sources/picture_ocr.docx"),
        )
        .expect("fixture")
    };
    let md = |c: DocumentConverter| {
        c.convert(src())
            .expect("convert")
            .document
            .export_to_markdown()
    };
    let default = md(DocumentConverter::new().do_picture_ocr(true));
    let tuned = md(DocumentConverter::new()
        .do_picture_ocr(true)
        .picture_ocr_scale(1.0)
        .ocr_textline_orientation(true));
    assert!(
        tuned.contains("Quarterly revenue grew by 12 percent"),
        "{tuned}"
    );
    assert_eq!(tuned, default);
}
