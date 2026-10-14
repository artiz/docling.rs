//! The MCP catalog metadata (#669) holds together: `server.json` (the
//! official registry), the PyPI launcher it lists, the image label the
//! registry checks, the MCP bundle Smithery lists (`crates/docling-py/mcp/mcpb`,
//! named by `smithery.yaml`) and `glama.json` name the same server, stay
//! inside the catalogs' limits, and pass only flags the server takes. A
//! mismatch fails a catalog submission at release time, far from the change
//! that caused it.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

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

fn manifest() -> Value {
    serde_json::from_str(&read("crates/docling-py/mcp/mcpb/manifest.json"))
        .expect("manifest.json parses")
}

fn server_help() -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_docling-rs"))
        .args(["mcp", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Every flag the catalogs pass is one the server takes.
#[test]
fn catalog_arguments_are_server_flags() {
    let help = server_help();
    let s = server();
    for p in s["packages"].as_array().unwrap() {
        for arg in p["packageArguments"].as_array().into_iter().flatten() {
            if let Some(flag) = arg["name"].as_str() {
                assert!(help.contains(flag), "server.json passes {flag}");
            }
        }
    }
    let m = manifest();
    for arg in m["server"]["mcp_config"]["args"].as_array().unwrap() {
        let arg = arg.as_str().unwrap();
        if arg.starts_with("--") {
            let flag = arg.split_once('=').map_or(arg, |(flag, _)| flag);
            assert!(help.contains(flag), "the bundle passes {flag}");
        }
    }
    let glama: Value = serde_json::from_str(&read("glama.json")).unwrap();
    assert!(!glama["maintainers"].as_array().unwrap().is_empty());
}

/// The bundle as Smithery's CLI (4.x) reads it: `smithery.yaml` names a
/// server id its rule accepts, the runtime is one it detects (`python` —
/// MCPB's newer `uv` type is rejected), the command runs the launcher at
/// the bundle's own version (mcp-registry.yml rewrites both together), and
/// every `${user_config.*}` the command uses is a declared setting. Smithery
/// substitutes only settings that have a value, after filling defaults, so
/// an optional setting without a default would reach the server as the
/// literal `${user_config.…}`.
#[test]
fn the_mcp_bundle_runs_the_launcher() {
    let m = manifest();
    let id = read("smithery.yaml")
        .lines()
        .find_map(|l| l.strip_prefix("name: "))
        .expect("smithery.yaml names the server")
        .trim()
        .to_string();
    assert_eq!(m["name"], id.as_str());
    let mut chars = id.chars();
    assert!(
        (3..=39).contains(&id.len())
            && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "Smithery rejects the server id {id:?}"
    );
    assert_eq!(m["server"]["type"], "python");
    let config = &m["server"]["mcp_config"];
    assert_eq!(config["command"], "uvx");
    let args: Vec<&str> = config["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(
        args[0],
        format!("docling-rs-mcp@{}", m["version"].as_str().unwrap())
    );
    let mut used = BTreeSet::new();
    let env = config["env"].as_object().into_iter().flatten();
    for text in args
        .iter()
        .copied()
        .chain(env.map(|(_, v)| v.as_str().unwrap()))
    {
        let mut rest = text;
        while let Some(at) = rest.find("${user_config.") {
            let key = &rest[at + "${user_config.".len()..];
            let end = key.find('}').expect("closed placeholder");
            used.insert(key[..end].to_string());
            rest = &key[end..];
        }
    }
    let settings = m["user_config"].as_object().unwrap();
    let declared: BTreeSet<String> = settings.keys().cloned().collect();
    assert_eq!(used, declared, "settings used vs. declared");
    for (key, setting) in settings {
        assert!(
            setting["required"] == true || setting.get("default").is_some(),
            "{key}: optional without a default"
        );
    }
    let icon = std::fs::read(
        repo()
            .join("crates/docling-py/mcp/mcpb")
            .join(m["icon"].as_str().unwrap()),
    )
    .expect("the bundle's icon");
    assert!(icon.starts_with(b"\x89PNG"), "MCPB icons are PNG");
}

/// The tools the bundle advertises (Smithery's listing shows them) are the
/// ones the server lists.
#[test]
fn the_mcp_bundle_lists_the_server_tools() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_docling-rs"))
        .args(["mcp", "--root"])
        .arg(repo())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    for message in [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "mcp-registry-test", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    ] {
        writeln!(stdin, "{message}").unwrap();
    }
    stdin.flush().unwrap();
    let listed: BTreeSet<String> = loop {
        let mut line = String::new();
        assert!(
            stdout.read_line(&mut line).unwrap() > 0,
            "server closed stdout"
        );
        let message: Value = serde_json::from_str(&line).unwrap();
        if message["id"] == 2 {
            break message["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap().to_string())
                .collect();
        }
    };
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    let advertised: BTreeSet<String> = manifest()["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(advertised, listed);
}
