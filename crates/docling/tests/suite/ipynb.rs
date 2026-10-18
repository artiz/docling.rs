//! Jupyter notebooks (#673): MarkItDown's `test_notebook.ipynb` and three
//! course notebooks, read as the Markdown their cells are — and, on
//! request, with their outputs.

use std::path::{Path, PathBuf};

use docling::{ConvertOptions, DocumentConverter, InputFormat, IpynbOutputs, SourceDocument};
use docling_core::Node;

fn data(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/ipynb/sources")
        .join(rel)
}

fn convert_with(rel: &str, outputs: IpynbOutputs) -> docling::ConversionResult {
    let source = SourceDocument::from_file(data(rel)).expect("fixture");
    DocumentConverter::new()
        .ipynb_outputs(outputs)
        .convert(source)
        .expect("converts")
}

fn convert(rel: &str) -> docling::ConversionResult {
    convert_with(rel, IpynbOutputs::None)
}

/// `(language, text)` of every code item, in order.
fn code_items(doc: &docling_core::DoclingDocument) -> Vec<(Option<String>, String)> {
    doc.nodes
        .iter()
        .filter_map(|n| match n {
            Node::Code { language, text, .. } => Some((language.clone(), text.clone())),
            _ => None,
        })
        .collect()
}

fn pictures(doc: &docling_core::DoclingDocument) -> usize {
    doc.nodes
        .iter()
        .filter(|n| matches!(n, Node::Picture { .. }))
        .count()
}

/// The issue's acceptance: MarkItDown's notebook comes out with its title
/// heading, a Python code fence holding the `print`, the "Code Cell Below"
/// section — and the JSON carries code items with a language.
#[test]
fn markitdown_notebook_matches_the_acceptance() {
    let r = convert("test_notebook.ipynb");
    assert_eq!(r.format, InputFormat::Ipynb);
    let md = r.document.export_to_markdown();
    assert!(md.starts_with("# Test Notebook\n"), "{md}");
    assert!(md.contains("```\nprint(\"markitdown\")\n```"), "{md}");
    assert!(md.contains("## Code Cell Below"), "{md}");
    assert!(md.contains("# comment in code\nprint(42)"), "{md}");
    assert!(md.trim_end().ends_with("End"), "{md}");
    // Strict Markdown keeps the fence language.
    let strict = DocumentConverter::new()
        .strict(true)
        .convert(SourceDocument::from_file(data("test_notebook.ipynb")).unwrap())
        .unwrap()
        .document
        .export_to_markdown_with(true);
    assert!(
        strict.contains("```python\nprint(\"markitdown\")"),
        "{strict}"
    );

    let codes = code_items(&r.document);
    assert_eq!(codes.len(), 2, "{codes:?}");
    assert!(codes.iter().all(|(l, _)| l.as_deref() == Some("python")));
    let json = r.document.export_to_json();
    assert!(json.contains("\"code_language\": \"Python\""), "{json}");
    // The outputs (`markitdown`, `42` on stdout) are not in the document.
    assert!(!md.contains("\n42\n"), "{md}");
    assert_eq!(pictures(&r.document), 0);
}

/// `ipynb_outputs=text`: stream and `text/plain` results follow their cell
/// as code blocks, image outputs become pictures with their bytes; the
/// default keeps none of them.
#[test]
fn text_outputs_keep_streams_results_and_pictures() {
    let quiet = convert("recommendations.ipynb");
    let md = quiet.document.export_to_markdown();
    assert!(
        md.starts_with("# Задание по программированию: Рекомендательные системы"),
        "{md}"
    );
    assert!(!md.contains("recommend_by_views_train.txt : 0.44"), "{md}");

    let loud = convert_with("recommendations.ipynb", IpynbOutputs::Text);
    let md = loud.document.export_to_markdown();
    assert!(
        md.contains("recommend_by_views_train.txt : 0.44 0.51 0.82 0.21"),
        "{md}"
    );
    // An `execute_result` keeps its `text/plain`.
    assert!(md.contains("array([ 0.33,  1.  ,  0.67,  0.4 ])"), "{md}");
    assert!(code_items(&loud.document).len() > code_items(&quiet.document).len());

    // Plots: five `image/png` outputs, read as pictures with their bytes
    // under the `embedded` tier the notebook's own outputs get.
    let quiz = convert_with("milk_quiz.ipynb", IpynbOutputs::Text);
    assert_eq!(pictures(&quiz.document), 5, "{:?}", quiz.document.nodes);
    let (html, _) = quiz
        .document
        .export_to_html_with_images(docling::ImageMode::Embedded, "artifacts");
    assert!(
        html.matches("data:image/png;base64,").count() >= 5,
        "{html}"
    );
    // The DataFrame result prints as its text, not its HTML.
    let md = quiz.document.export_to_markdown();
    assert!(md.contains("01.01.1962"), "{md}");
    assert!(!md.contains("<table"), "{md}");
    assert_eq!(pictures(&convert("milk_quiz.ipynb").document), 0);
}

/// The kernel names the language; raw cells are plain code blocks; a
/// `metadata.title` leads a notebook that does not open with a heading;
/// trailing empty cells leave nothing behind.
#[test]
fn language_raw_cells_and_title_follow_the_metadata() {
    let notebook = serde_json::json!({
        "nbformat": 4, "nbformat_minor": 5,
        "metadata": {"title": "Lab 1", "kernelspec": {"language": "R", "name": "ir"}},
        "cells": [
            {"cell_type": "markdown", "source": "Setup first.", "metadata": {}},
            {"cell_type": "code", "source": ["x <- c(1, 2)\n", "mean(x)"], "metadata": {}, "outputs": []},
            {"cell_type": "raw", "source": "raw: text", "metadata": {}},
            {"cell_type": "code", "source": "", "metadata": {}, "outputs": []}
        ]
    });
    let source = SourceDocument::from_bytes(
        "lab.ipynb",
        InputFormat::Ipynb,
        notebook.to_string().into_bytes(),
    );
    let doc = DocumentConverter::new().convert(source).unwrap().document;
    let md = doc.export_to_markdown();
    assert!(md.starts_with("# Lab 1\n\nSetup first.\n"), "{md}");
    let codes = code_items(&doc);
    assert_eq!(
        codes,
        vec![
            (Some("R".into()), "x <- c(1, 2)\nmean(x)".into()),
            (None, "raw: text".into())
        ]
    );
}

/// Display formulas are formula items — `$$latex$$` in Markdown, a
/// `formula` item in JSON, MathML in HTML — whether on one line or three;
/// inline math, dollar amounts and a formula inside a link stay prose.
#[test]
fn display_formulas_become_formula_items() {
    let r = convert("cross_entropy_notes.ipynb");
    let formulas: Vec<&str> = r
        .document
        .nodes
        .iter()
        .filter_map(|n| match n {
            Node::Formula { latex, .. } => Some(latex.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(formulas.len(), 2, "{:?}", r.document.nodes);
    assert!(
        formulas[0].starts_with("H(y) = \\sum_i y_i"),
        "{}",
        formulas[0]
    );
    assert!(
        formulas[1].contains("\\mbox{KL}(y~||~\\hat{y})\n= \\sum_i"),
        "{}",
        formulas[1]
    );

    let md = r.document.export_to_markdown();
    assert!(
        md.contains("$$H(y) = \\sum_i y_i \\log \\frac{1}{y_i} = -\\sum_i y_i \\log y_i$$"),
        "{md}"
    );
    assert!(md.contains("distribution $y$ is the expected"), "{md}");
    assert!(md.contains("$0.10 per bit and $5 per message"), "{md}");
    assert!(
        md.contains("[$$E=mc^2$$](https://en.wikipedia.org/wiki/Mass%E2%80%93energy_equivalence)"),
        "{md}"
    );

    let json = r.document.export_to_json();
    assert_eq!(json.matches("\"label\": \"formula\"").count(), 2, "{json}");
    let (html, _) = r
        .document
        .export_to_html_with_images(docling::ImageMode::Placeholder, "artifacts");
    assert!(html.matches("<math").count() >= 2, "{html}");
    assert!(html.contains("annotation encoding=\"TeX\""), "{html}");
    assert_eq!(code_items(&r.document).len(), 1);
}

/// A UTF-8 BOM in front of the JSON, a notebook under `.json`, the media
/// type, and the format id all reach the notebook backend.
#[test]
fn bom_json_extension_and_media_type_route_to_the_notebook() {
    let bytes = std::fs::read(data("test_notebook.ipynb")).unwrap();
    let mut with_bom = b"\xEF\xBB\xBF".to_vec();
    with_bom.extend_from_slice(&bytes);
    let source = SourceDocument::from_bytes("nb.ipynb", InputFormat::Ipynb, with_bom.clone());
    let md = DocumentConverter::new()
        .convert(source)
        .unwrap()
        .document
        .export_to_markdown();
    assert!(md.starts_with("# Test Notebook\n"), "{md}");

    // `.json`: the docling-JSON route sniffs the notebook.
    let source = SourceDocument::from_bytes("nb.json", InputFormat::JsonDocling, with_bom);
    let r = DocumentConverter::new().convert(source).unwrap();
    let md = r.document.export_to_markdown();
    assert!(md.contains("## Code Cell Below"), "{md}");
    assert_eq!(code_items(&r.document).len(), 2);

    // Nameless bytes by media type; the id and the extension.
    let source =
        SourceDocument::from_named_bytes("stdin", bytes, Some("application/x-ipynb+json")).unwrap();
    assert_eq!(source.format, InputFormat::Ipynb);
    assert_eq!(InputFormat::from_id("ipynb"), Some(InputFormat::Ipynb));
    assert_eq!(
        InputFormat::from_extension("ipynb"),
        Some(InputFormat::Ipynb)
    );
    assert_eq!(InputFormat::Ipynb.as_str(), "ipynb");
    assert!(InputFormat::supported_extensions().contains(&"ipynb"));
}

/// Broken notebooks fail as such; the option's spelling is validated with
/// the others.
#[test]
fn errors_name_the_notebook_and_the_option() {
    let source = SourceDocument::from_bytes(
        "x.ipynb",
        InputFormat::Ipynb,
        b"{\"nbformat\": 4, \"cells\": [".to_vec(),
    );
    let e = DocumentConverter::new().convert(source).unwrap_err();
    assert!(e.to_string().contains("ipynb"), "{e}");
    let source = SourceDocument::from_bytes("x.ipynb", InputFormat::Ipynb, b"[1, 2]".to_vec());
    let e = DocumentConverter::new().convert(source).unwrap_err();
    assert!(e.to_string().contains("not a JSON object"), "{e}");

    let bad = ConvertOptions {
        ipynb_outputs: Some("all".into()),
        ..Default::default()
    };
    let e = bad.validate().unwrap_err();
    assert_eq!(
        e.cli_message(),
        "--ipynb-outputs must be one of none, text; got \"all\""
    );
    let ok = ConvertOptions {
        ipynb_outputs: Some("Text".into()),
        ..Default::default()
    };
    assert_eq!(ok.ipynb_outputs().unwrap(), Some(IpynbOutputs::Text));
}
