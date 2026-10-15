//! Content-based format detection (#672): `InputFormat::detect` over the
//! corpus, and the nameless / misnamed inputs it routes — a PPTX renamed
//! to nothing, a PDF saved as `.bin`.

use std::path::{Path, PathBuf};

use docling::{DocumentConverter, InputFormat, SourceDocument};

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn detect(rel: &str) -> Option<InputFormat> {
    let bytes = std::fs::read(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    InputFormat::detect(&bytes)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("docling-detect-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Every signature the sniffer knows, on the corpus: packages, OLE files,
/// PDF, RTF, HTML, images, DjVu, audio and video, iWork, WordPerfect,
/// WebVTT, XML dialects, JSON and notebooks, MIME messages, LaTeX.
#[test]
fn corpus_files_are_detected_by_content() {
    use InputFormat::*;
    let cases: &[(&str, InputFormat)] = &[
        ("tests/data/docx/sources/lorem_ipsum.docx", Docx),
        ("tests/data/xlsx/sources/xlsx_01.xlsx", Xlsx),
        ("tests/data/pptx/sources/powerpoint_bad_text.pptx", Pptx),
        (
            "crates/docling/tests/data/visio/sources/blockdiagramm.vsdx",
            Visio,
        ),
        ("tests/data/odf/sources/text_document_01.odt", Odt),
        ("tests/data/odf/sources/odf_table_with_title_01.ods", Ods),
        ("tests/data/odf/sources/odf_presentation_01.odp", Odp),
        ("tests/data/epub/sources/epub_purvis_poetry.epub", Epub),
        ("tests/data/rtf/sources/docx_checkboxes.rtf", Rtf),
        ("tests/data/doc/sources/docx_lists.doc", Doc),
        ("tests/data/xls/sources/xlsx_01.xls", Xls),
        ("tests/data/ppt/sources/powerpoint_sample.ppt", Ppt),
        ("tests/data/pdf/sources/base14_fonts.pdf", Pdf),
        ("tests/data/html/sources/hyperlink_03.html", Html),
        ("tests/data/tiff/sources/2206.01062.tif", Image),
        ("tests/data/heif/sources/inspection_report.heic", Image),
        ("crates/docling/tests/data/djvu/sources/example.djvu", Djvu),
        ("tests/data/audio/sources/sample_10s.mp3", Audio),
        ("tests/data/audio/sources/sample_10s_audio-aac.aac", Audio),
        ("tests/data/audio/sources/sample_10s_audio-flac.flac", Audio),
        ("tests/data/audio/sources/sample_10s_audio-m4a.m4a", Audio),
        ("tests/data/audio/sources/sample_10s_audio-ogg.ogg", Audio),
        ("tests/data/audio/sources/sample_10s_audio-wav.wav", Audio),
        ("tests/data/audio/sources/sample_10s_video-mp4.mp4", Video),
        (
            "tests/data/audio/sources/sample_10s_video-quicktime.mov",
            Video,
        ),
        ("tests/data/audio/sources/sample_10s_video-mkv.mkv", Video),
        ("tests/data/audio/sources/sample_10s_video-webm.webm", Video),
        ("tests/data/audio/sources/sample_10s_video-avi.avi", Video),
        ("tests/data/iwork/sources/one_slide.key", Keynote),
        ("tests/data/iwork/sources/numeric_table.key", Keynote),
        ("tests/data/keynote/sources/keynote_2013.key", Keynote),
        (
            "tests/data/iwork/sources/account_statement.numbers",
            Numbers,
        ),
        ("tests/data/iwork/sources/two_sheets.numbers", Numbers),
        ("tests/data/iwork/sources/proposal_simple.pages", Pages),
        ("tests/data/iwork/sources/proposal_nested_dir.pages", Pages),
        ("tests/data/pages/sources/pages_2013.pages", Pages),
        ("tests/data/pages/sources/pages_iwork09.pages", Pages),
        (
            "crates/docling/tests/data/wordperfect/sources/formatcorpus_wp61_win.wpd",
            WordPerfect,
        ),
        (
            "crates/docling/tests/data/abiword/sources/word_sample.abw",
            Abiword,
        ),
        ("tests/data/webvtt/sources/webvtt_example_01.vtt", Vtt),
        ("tests/data/jats/sources/elife-56337.xml", Xml),
        ("tests/data/jats/sources/elife-56337.nxml", Xml),
        ("tests/data/xbrl/sources/mlac-20251231.xml", Xml),
        ("crates/docling/tests/data/text/sources/settings.xml", Xml),
        (
            "crates/docling/tests/data/ipynb/sources/test_notebook.ipynb",
            Ipynb,
        ),
        (
            "crates/docling/tests/data/json_docling/sources/docx_picture.json",
            JsonDocling,
        ),
        (
            "crates/docling/tests/data/text/sources/test.json",
            JsonDocling,
        ),
        ("tests/data/email/sources/eml_simple.eml", Email),
        ("tests/data/email/sources/eml_with_attachment.eml", Email),
        ("tests/data/email/sources/msg_simple.msg", Email),
        ("tests/data/mhtml/sources/example.mhtml", Mhtml),
        ("tests/data/latex/sources/0005057/diss.tex", Latex),
    ];
    let mut wrong = Vec::new();
    for (rel, want) in cases {
        let got = detect(rel);
        if got != Some(*want) {
            wrong.push(format!("{rel}: want {want:?}, got {got:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));

    // No signature: prose, Markdown, CSV, source code, JSON Lines — the
    // name (or the plain-text route) decides, not a guess.
    for rel in [
        "crates/docling/tests/data/text/sources/script.py",
        "crates/docling/tests/data/text/sources/app.log",
        "crates/docling/tests/data/text/sources/records.jsonl",
        "tests/data/csv/sources/csv-comma-in-cell.csv",
        "tests/data/asciidoc/sources/asciidoc_01.asciidoc",
    ] {
        assert_eq!(detect(rel), None, "{rel}");
    }
    assert_eq!(InputFormat::detect(b""), None);
    assert_eq!(InputFormat::detect(b"just some words\n"), None);
    assert_eq!(InputFormat::detect(&[0, 1, 2, 255, 254, 0]), None);
}

/// The text signatures on synthetic heads: DocTags, SVG, a bare feed root,
/// a declaration, a notebook, a message with folded headers, LaTeX.
#[test]
fn text_signatures() {
    use InputFormat::*;
    let cases: &[(&[u8], Option<InputFormat>)] = &[
        (b"<doctag><text><loc_1><loc_2><loc_3><loc_4>Hi</text></doctag>", Some(DocTags)),
        (b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"><rect/></svg>", Some(Svg)),
        (b"<svg viewBox=\"0 0 1 1\"></svg>", Some(Svg)),
        (b"<rss version=\"2.0\"><channel><title>t</title></channel></rss>", Some(Xml)),
        (b"<feed xmlns=\"http://www.w3.org/2005/Atom\"><title>t</title></feed>", Some(Xml)),
        (b"\xEF\xBB\xBF<?xml version=\"1.0\"?><article><front/></article>", Some(Xml)),
        (b"<abiword xmlns=\"http://www.abisource.com/awml.dtd\"><section/></abiword>", Some(Abiword)),
        (b"<!DOCTYPE html><html><body>x</body></html>", Some(Html)),
        (b"<b>bold</b> words", None),
        (b"<alice@example.com> wrote:", None),
        (b"{\"nbformat\": 4, \"nbformat_minor\": 5, \"cells\": [], \"metadata\": {}}", Some(Ipynb)),
        (b"{\"schema_name\": \"DoclingDocument\", \"body\": {}}", Some(JsonDocling)),
        (b"[1, 2, 3]", Some(JsonDocling)),
        (b"{\"broken\": ", None),
        (b"From: a@x.org\r\nTo: b@x.org\r\nSubject: hi\r\n\r\nbody", Some(Email)),
        (
            b"From: <Saved by Blink>\nSubject: page\nMIME-Version: 1.0\nContent-Type: multipart/related;\n\ttype=\"text/html\"\n\n--b\n",
            Some(Mhtml),
        ),
        (b"Title: a pandoc document\nAuthor: me\n\nText.", None),
        (b"WEBVTT\n\n00:00.000 --> 00:01.000\nhi", Some(Vtt)),
        (b"\\documentclass{article}\n\\begin{document}x\\end{document}", Some(Latex)),
        (b"\\section{Not a document}", None),
        (b"# Heading\n\nprose", None),
    ];
    for (bytes, want) in cases {
        assert_eq!(
            InputFormat::detect(bytes),
            *want,
            "{}",
            String::from_utf8_lossy(bytes)
        );
    }
}

/// The issue's acceptance: a PPTX renamed to a name with no extension
/// converts as the original; a PDF saved as `.bin` converts; a correctly
/// named file is never sniffed.
#[test]
fn nameless_and_misnamed_files_convert_by_content() {
    let pptx = repo("tests/data/pptx/sources/powerpoint_bad_text.pptx");
    let deck = scratch("deck");
    std::fs::copy(&pptx, &deck).unwrap();
    let named = SourceDocument::from_file(&pptx).unwrap();
    let nameless = SourceDocument::from_file(&deck).unwrap();
    assert_eq!(nameless.format, InputFormat::Pptx);
    assert_eq!(nameless.name, "deck");
    let want = DocumentConverter::new()
        .convert(named)
        .unwrap()
        .document
        .export_to_markdown();
    let got = DocumentConverter::new()
        .convert(nameless)
        .unwrap()
        .document
        .export_to_markdown();
    assert_eq!(got, want);
    assert!(!want.trim().is_empty());

    let pdf = repo("tests/data/pdf/sources/base14_fonts.pdf");
    let bin = scratch("scan.bin");
    std::fs::copy(&pdf, &bin).unwrap();
    let source = SourceDocument::from_file(&bin).unwrap();
    assert_eq!(source.format, InputFormat::Pdf);
    assert_eq!(source.extension.as_deref(), Some("bin"));
    let md = DocumentConverter::new()
        .text_layer_only(true)
        .convert(source)
        .unwrap()
        .document
        .export_to_markdown();
    assert!(md.contains("quick brown fox"), "{md}");

    // Nameless bytes with no media type: the same route.
    let bytes = std::fs::read(&pdf).unwrap();
    let source = SourceDocument::from_named_bytes("upload", bytes, None).unwrap();
    assert_eq!(source.format, InputFormat::Pdf);

    // A `.md` holding a PDF is still read by its name first — detection
    // never second-guesses a known extension up front (#556 retries only
    // when the conversion fails).
    let lying = scratch("notes.md");
    std::fs::copy(&pdf, &lying).unwrap();
    assert_eq!(
        SourceDocument::from_file(&lying).unwrap().format,
        InputFormat::Md
    );
}
