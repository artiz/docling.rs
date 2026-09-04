//! CLI surface tests: the flags a script or a container smoke test reaches for
//! before any document exists. They run the real binary, so they also pin that
//! `--help`/`--version` answer with **no models and no arguments** — the case
//! that broke the CUDA image smoke test in issue #333.

use std::process::Command;

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_docling-rs"))
        .args(args)
        .output()
        .expect("run docling-rs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `--version` / `-V`: exit 0, the crate version on stdout, nothing on stderr.
#[test]
fn version_flag_reports_the_crate_version() {
    for flag in ["--version", "-V"] {
        let (code, stdout, stderr) = run(&[flag]);
        assert_eq!(code, 0, "{flag}: stderr: {stderr}");
        assert!(
            stdout.starts_with(&format!("docling-rs {}", env!("CARGO_PKG_VERSION"))),
            "{flag}: {stdout:?}"
        );
        assert!(stderr.is_empty(), "{flag}: stderr: {stderr:?}");
    }
}

/// The version line names the optional features the binary carries, so a bug
/// report says which execution providers were even compiled in.
#[test]
fn version_lists_compiled_features() {
    let (_, stdout, _) = run(&["--version"]);
    #[cfg(feature = "chunking")]
    assert!(stdout.contains("chunking"), "{stdout:?}");
    #[cfg(not(feature = "chunking"))]
    assert!(!stdout.contains('('), "{stdout:?}");
}

/// `--help` / `-h`: exit 0, the flag list on stdout (not stderr — it is the
/// requested output, not a diagnostic).
#[test]
fn help_flag_prints_the_flag_list() {
    for flag in ["--help", "-h"] {
        let (code, stdout, stderr) = run(&[flag]);
        assert_eq!(code, 0, "{flag}: stderr: {stderr}");
        assert!(stdout.contains("usage: docling-rs"), "{flag}: {stdout:?}");
        for expected in ["--to md|json", "--pages A-B", "--pipeline standard|vlm"] {
            assert!(stdout.contains(expected), "{flag}: missing {expected}");
        }
        assert!(stderr.is_empty(), "{flag}: stderr: {stderr:?}");
    }
}

/// No arguments is a usage error, not a panic or a silent success.
#[test]
fn no_arguments_is_a_usage_error() {
    let (code, stdout, stderr) = run(&[]);
    assert_eq!(code, 2);
    assert!(stdout.is_empty(), "{stdout:?}");
    assert!(stderr.contains("no input file"), "{stderr:?}");
    assert!(stderr.contains("--help"), "{stderr:?}");
}

/// An unknown flag names itself and points at `--help`.
#[test]
fn unknown_flag_points_at_help() {
    let (code, _, stderr) = run(&["--no-such-flag"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("--no-such-flag"), "{stderr:?}");
    assert!(stderr.contains("--help"), "{stderr:?}");
}

/// `--help` after other flags still prints help rather than treating the flag
/// as a file name — a smoke test may append it to a canned argument list.
#[test]
fn help_is_recognized_in_any_position() {
    let (code, stdout, _) = run(&["--strict", "--to", "json", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("usage: docling-rs"), "{stdout:?}");
}
