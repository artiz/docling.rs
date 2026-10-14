//! The MCP catalog metadata (#669) holds together: `server.json` (the
//! official registry), the PyPI launcher it lists, the image label the
//! registry checks, `smithery.yaml` and `glama.json` name the same server,
//! stay inside the registry's limits, and pass only flags the server takes.
//! A mismatch fails a registry submission at release time, far from the
//! change that caused it.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

const NAME: &str = "io.github.docling-project/docling-rs";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn server() -> Value {
    serde_json::from_str(&read("server.json")).expect("server.json parses")
}

#[test]
fn server_json_is_within_the_registry_limits() {
    let s = server();
    assert_eq!(s["name"], NAME);
    // The schema's caps (2025-12-11): description and title ≤ 100.
    for key in ["description", "title"] {
        let v = s[key].as_str().unwrap();
        assert!(!v.is_empty() && v.chars().count() <= 100, "{key}: {v}");
    }
    let packages = s["packages"].as_array().unwrap();
    let pypi = packages
        .iter()
        .find(|p| p["registryType"] == "pypi")
        .unwrap();
    assert_eq!(pypi["identifier"], "docling-rs-mcp");
    assert_eq!(pypi["runtimeHint"], "uvx");
    let oci = packages
        .iter()
        .find(|p| p["registryType"] == "oci")
        .unwrap();
    assert!(oci["identifier"]
        .as_str()
        .unwrap()
        .starts_with("ghcr.io/docling-project/docling-rs:"));
}

/// The registry accepts a PyPI package whose README (its PyPI description)
/// carries `mcp-name: <name>` followed by whitespace or the comment's end,
/// and an image labelled with the name.
#[test]
fn the_listed_packages_carry_the_ownership_markers() {
    let readme = read("crates/docling-py/mcp/README.md");
    let marker = format!("mcp-name: {NAME}");
    let at = readme
        .find(&marker)
        .expect("launcher README carries the marker");
    let next = readme[at + marker.len()..].chars().next();
    assert!(matches!(next, Some(' ' | '\n' | '-')), "{next:?}");
    let pyproject = read("crates/docling-py/mcp/pyproject.toml");
    assert!(pyproject.contains("name = \"docling-rs-mcp\""));
    assert!(pyproject.contains("docling-rs-mcp = \"docling_rs_mcp:main\""));
    assert!(pyproject.contains("\"docling-rs=="));
    let dockerfile = read("crates/docling-serve/Dockerfile");
    let label = format!("LABEL io.modelcontextprotocol.server.name=\"{NAME}\"");
    for stage in [
        "FROM runtime AS cli\n",
        "FROM runtime AS cli-s390x\n",
        "FROM runtime-cuda AS cli-cuda\n",
    ] {
        let at = dockerfile
            .find(stage)
            .unwrap_or_else(|| panic!("{stage:?}"))
            + stage.len();
        let end = dockerfile[at..]
            .find("\nFROM ")
            .map_or(dockerfile.len(), |e| at + e);
        assert!(
            dockerfile[at..end].contains(&label),
            "{stage:?} lacks the label"
        );
    }
}

/// Every flag the catalogs pass is one the server takes.
#[test]
fn catalog_arguments_are_server_flags() {
    let out = Command::new(env!("CARGO_BIN_EXE_docling-rs"))
        .args(["mcp", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    let s = server();
    for p in s["packages"].as_array().unwrap() {
        for arg in p["packageArguments"].as_array().into_iter().flatten() {
            if let Some(flag) = arg["name"].as_str() {
                assert!(help.contains(flag), "server.json passes {flag}");
            }
        }
    }
    let smithery = read("smithery.yaml");
    assert!(smithery.contains("command: 'uvx'"));
    assert!(smithery.contains("'docling-rs-mcp'"));
    for flag in ["--root", "--allow-url-fetch"] {
        assert!(smithery.contains(flag) && help.contains(flag), "{flag}");
    }
    let glama: Value = serde_json::from_str(&read("glama.json")).unwrap();
    assert!(!glama["maintainers"].as_array().unwrap().is_empty());
}
