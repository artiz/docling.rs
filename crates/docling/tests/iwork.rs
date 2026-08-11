//! Apple iWork input (#213): the IWA backend pinned against committed
//! Markdown groundtruth over `tests/data/iwork/` (repo root). The backend is
//! pure Rust and deterministic — no models, no gating. Regenerate after an
//! intentional change with:
//!
//! ```bash
//! DOCLING_RS_REGEN=1 cargo test -p docling --test iwork
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use docling::{DocumentConverter, SourceDocument};

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/iwork")
}

#[test]
fn iwork_fixtures_match_groundtruth() {
    let regen = std::env::var_os("DOCLING_RS_REGEN").is_some();
    let sources = corpus().join("sources");
    let mut checked = 0;
    let mut entries: Vec<_> = fs::read_dir(&sources)
        .expect("iwork sources")
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();
    for path in entries {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let source = SourceDocument::from_file(&path).expect("iwork fixture");
        let md = DocumentConverter::new()
            .convert(source)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .document
            .export_to_markdown();
        let gt_path = corpus().join("groundtruth").join(format!("{name}.md"));
        if regen {
            fs::write(&gt_path, &md).expect("write groundtruth");
        } else {
            let expected = fs::read_to_string(&gt_path)
                .unwrap_or_else(|_| panic!("{name}: missing groundtruth (DOCLING_RS_REGEN=1)"));
            assert_eq!(md, expected, "{name}: Markdown drifted from groundtruth");
        }
        checked += 1;
    }
    assert!(
        checked >= 6,
        "expected the full iwork corpus, saw {checked}"
    );
}

/// Legacy (pre-2013) iWork packages carry `index.xml`, not IWA — the error
/// must say so instead of a generic parse failure.
#[test]
fn pre_iwa_package_reports_clearly() {
    // A minimal zip with only an index.xml member.
    let mut buf = Vec::new();
    {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        zip.start_file::<_, ()>("index.xml", Default::default())
            .unwrap();
        zip.write_all(b"<document/>").unwrap();
        zip.finish().unwrap();
    }
    let source = SourceDocument::from_bytes("old", docling::InputFormat::Pages, buf);
    let err = DocumentConverter::new().convert(source).unwrap_err();
    assert!(
        err.to_string().contains("pre-2013"),
        "unexpected error: {err}"
    );
}
