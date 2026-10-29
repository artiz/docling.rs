//! Every path in the repository checks out on Windows: no component is a
//! reserved device name (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`,
//! `LPT1`–`LPT9` — with any extension, `prn.rs` included), none holds a
//! character Windows forbids or ends in a dot or a space. A `prn.rs`
//! module once failed every Windows CI job at `git checkout` (#216).

use std::path::Path;

fn portable(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").to_ascii_lowercase();
    let reserved = matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0');
    !reserved
        && !name
            .chars()
            .any(|c| "<>:\"|?*\\".contains(c) || c.is_control())
        && !name.ends_with(['.', ' '])
}

fn walk(dir: &Path, bad: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Build output, virtualenvs and dependency caches are not
        // checked in.
        if name.starts_with('.') && name != ".github"
            || matches!(name.as_str(), "target" | "node_modules" | "__pycache__")
        {
            continue;
        }
        let path = entry.path();
        if !portable(&name) {
            bad.push(path.display().to_string());
        }
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&path, bad);
        }
    }
}

#[test]
fn repository_paths_check_out_on_windows() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut bad = Vec::new();
    for top in ["crates", "docs", "scripts", "tests", ".github"] {
        walk(&root.join(top), &mut bad);
    }
    assert!(bad.is_empty(), "paths Windows cannot check out: {bad:#?}");
}

#[test]
fn reserved_names_are_caught() {
    for name in [
        "prn.rs", "PRN", "con.txt", "aux", "nul.json", "com1.md", "LPT9",
    ] {
        assert!(!portable(name), "{name}");
    }
    for name in [
        "print_file.rs",
        "prnt.rs",
        "com0.md",
        "console.rs",
        "lotus_report.prn",
    ] {
        assert!(portable(name), "{name}");
    }
}
