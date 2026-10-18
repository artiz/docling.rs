//! The option inventory (#577): every field of [`docling::ConvertOptions`]
//! is documented on every surface that exposes it, in that surface's own
//! spelling — the CLI help, docling-serve's OpenAPI document, the Python
//! wrapper's keyword arguments, the Node TypeScript declarations, the C ABI
//! README and the central table `docs/OPTIONS.md`. [`docling::OPTIONS`] says
//! where a surface spells an option differently or does not have it, with
//! the reason next to the row; the struct and that table are checked against
//! each other in the library's own unit tests. A field added to the struct
//! therefore fails here until each surface carries it — the drift this
//! refactor removed cannot silently come back.

use std::path::{Path, PathBuf};

use docling::{cli_flag, ConvertOptions, OptionInfo, OPTIONS};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `ocr_lang` → `ocrLang`, napi's rename of the option fields.
fn camel_case(snake: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in snake.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// The surface's spelling from a row: `None` = the default, `Some("")` =
/// the surface has no such option.
fn spelling(explicit: Option<&str>, default: impl FnOnce() -> String) -> Option<String> {
    match explicit {
        Some("") => None,
        Some(s) => Some(s.to_string()),
        None => Some(default()),
    }
}

fn each_option(check: impl Fn(&OptionInfo) -> Option<String>) {
    let missing: Vec<String> = OPTIONS.iter().filter_map(check).collect();
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

/// Every option is a CLI flag in the help text.
#[test]
fn cli_help_lists_every_option() {
    let help = read("crates/docling-cli/src/main.rs");
    each_option(|o| {
        let flag = cli_flag(o.name);
        (!help.contains(&flag)).then(|| format!("CLI help lacks {flag} ({})", o.name))
    });
}

/// Every option is a docling-serve request option: a query parameter and a
/// multipart form field in the OpenAPI document (which is what the JSON body
/// accepts too). The server's historical `md_page_break_placeholder` stays
/// documented as the alias it is.
#[test]
fn openapi_lists_every_option() {
    let spec = read("crates/docling-serve/src/openapi.yaml");
    each_option(|o| {
        let name = o.name;
        let mut missing = Vec::new();
        if !spec.contains(&format!("{{ name: {name}, in: query")) {
            missing.push(format!("openapi.yaml has no query parameter {name}"));
        }
        if !spec.contains(&format!("\n                {name}:")) {
            missing.push(format!("openapi.yaml multipart schema lacks {name}"));
        }
        (!missing.is_empty()).then(|| missing.join("\n"))
    });
    assert!(
        spec.contains("{ name: md_page_break_placeholder, in: query"),
        "the historical md_page_break_placeholder alias must stay documented"
    );
}

/// Every option the Python surface has is a keyword argument of both the
/// native `DocumentConverter` and the `docling_rs.DocumentConverter` wrapper.
#[test]
fn python_converter_takes_every_option() {
    let native = read("crates/docling-py/src/lib.rs");
    let wrapper = read("crates/docling-py/python/docling_rs/__init__.py");
    each_option(|o| {
        let kwarg = spelling(o.python, || o.name.to_string())?;
        let mut missing = Vec::new();
        if !native.contains(&format!("        {kwarg} = ")) {
            missing.push(format!("docling-py native signature lacks {kwarg}"));
        }
        if !wrapper.contains(&format!("        {kwarg}: ")) {
            missing.push(format!("docling_rs.DocumentConverter lacks {kwarg}"));
        }
        (!missing.is_empty()).then(|| missing.join("\n"))
    });
}

/// Every option the Node surface has is a property of both napi option
/// objects — `ConverterOptions` (the `DocumentConverter` / `Pipeline`
/// constructors) and `ConvertOptions` (the one-shot functions) — read off
/// their Rust definitions: napi generates `native.d.ts` from them at build
/// time (it is not checked in), spelling each field in camelCase.
#[test]
fn node_option_objects_declare_every_option() {
    let src = read("crates/docling-node/src/lib.rs");
    let struct_body = |name: &str| -> &str {
        let start = src
            .find(&format!("pub struct {name} {{"))
            .unwrap_or_else(|| panic!("docling-node has no `pub struct {name}`"));
        let end = src[start..].find("\n}\n").expect("struct end") + start;
        &src[start..end]
    };
    // The class takes its options in two halves: the constructor's
    // `ConverterOptions` and each call's `OutputOptions` (where the
    // Markdown-export choices such as `pageBreakPlaceholder` live).
    let converter = format!(
        "{}\n{}",
        struct_body("ConverterOptions"),
        struct_body("OutputOptions")
    );
    let one_shot = struct_body("ConvertOptions");
    each_option(|o| {
        // The row's spelling is the TypeScript one; the Rust field is the
        // wire name itself (napi renames it).
        spelling(o.node, || camel_case(o.name))?;
        let field = format!("    pub {}: Option<", o.name);
        let mut missing = Vec::new();
        if !converter.contains(&field) {
            missing.push(format!(
                "docling-node ConverterOptions/OutputOptions lack {}",
                camel_case(o.name)
            ));
        }
        if !one_shot.contains(&field) {
            missing.push(format!(
                "docling-node ConvertOptions lacks {}",
                camel_case(o.name)
            ));
        }
        (!missing.is_empty()).then(|| missing.join("\n"))
    });
}

/// Every option is a documented JSON key of the C ABI (the wasm module takes
/// the same object and points at the same table).
#[test]
fn ffi_readme_documents_every_option() {
    let readme = read("crates/docling-ffi/README.md");
    each_option(|o| {
        (!readme.contains(&format!("`{}`", o.name)))
            .then(|| format!("docling-ffi/README.md lacks `{}`", o.name))
    });
}

/// `docs/OPTIONS.md` — the human table — has a row per option.
#[test]
fn options_doc_has_every_row() {
    let doc = read("docs/OPTIONS.md");
    each_option(|o| {
        (!doc.contains(&format!("| `{}` |", o.name)))
            .then(|| format!("docs/OPTIONS.md has no row for `{}`", o.name))
    });
    // And no row for an option that does not exist.
    let fields = ConvertOptions::field_names();
    for line in doc.lines().filter(|l| l.starts_with("| `")) {
        let name = line
            .trim_start_matches("| `")
            .split('`')
            .next()
            .unwrap_or("");
        if name.contains(' ') || name.is_empty() {
            continue;
        }
        assert!(
            fields.iter().any(|f| f == name),
            "docs/OPTIONS.md documents `{name}`, which is not a ConvertOptions field"
        );
    }
}
