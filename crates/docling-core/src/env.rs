//! Environment-variable helpers shared by every crate in the workspace.
//!
//! The knobs documented in the README (`DOCLING_RS_*`, `DOCLING_*`) grew one
//! hand-rolled `std::env::var` dance per call site — presence checks that
//! treated `FOO=0` as *on*, three copies of the same truthiness predicate,
//! a dozen `.ok().and_then(|v| v.parse().ok())` chains. This module is the
//! single vocabulary for all of them:
//!
//! - [`flag`] — boolean knobs (`DOCLING_RS_FP32=1`);
//! - [`nonempty`] — string knobs where a blank value means "unset";
//! - [`parse`] — numeric knobs with a coded default;
//! - [`debug_enabled`] / [`crate::debug_log!`] — the `DOCLING_RS_DEBUG`
//!   diagnostics channel.
//!
//! On targets without an environment (wasm32-unknown-unknown) `std::env::var`
//! reports "not present", so every helper falls back to its default — the
//! wasm builds keep compiling with no cfg noise at the call sites.

/// True when `key` is set to a truthy value. Truthy is anything except the
/// explicit "off" spellings — empty, `0`, `false`, `no`, `off` (trimmed,
/// ASCII case-insensitive) — so both `FOO=1` and `FOO=yes` enable, and
/// `FOO=0` actually disables instead of counting as "present, therefore on"
/// (the trap the old `env::var(..).is_ok()` checks all shared).
pub fn flag(key: &str) -> bool {
    match std::env::var(key) {
        Ok(v) => {
            let v = v.trim();
            !(v.is_empty()
                || v == "0"
                || v.eq_ignore_ascii_case("false")
                || v.eq_ignore_ascii_case("no")
                || v.eq_ignore_ascii_case("off"))
        }
        Err(_) => false,
    }
}

/// The trimmed value of `key`, if set and non-blank. The `Option` shape makes
/// "env override, else default" read as `nonempty(K).unwrap_or_else(..)` and
/// composes with `.or_else` chains for multi-variable fallbacks.
pub fn nonempty(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(v) => {
            let v = v.trim();
            (!v.is_empty()).then(|| v.to_string())
        }
        Err(_) => None,
    }
}

/// `key` parsed as `T`, if set and parseable. Unparseable values fall back to
/// the coded default silently — tuning knobs degrade, they don't error.
pub fn parse<T: std::str::FromStr>(key: &str) -> Option<T> {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok())
}

/// Whether `DOCLING_RS_DEBUG` diagnostics are on. Cached on first use: the
/// callers sit inside per-page pipeline loops, and a process does not
/// meaningfully flip its own debug env mid-run.
pub fn debug_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| flag("DOCLING_RS_DEBUG"))
}

/// `eprintln!` gated on [`env::debug_enabled`](debug_enabled) — the quiet
/// diagnostics channel (`DOCLING_RS_DEBUG=1`). Callers keep their own
/// `docling-<crate>:` message prefixes; the macro only owns the gate.
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if $crate::env::debug_enabled() {
            eprintln!($($arg)*);
        }
    };
}

#[cfg(test)]
mod tests {
    // Env mutation is process-global and the test harness is parallel, so
    // every test owns uniquely-named variables and nothing else reads them.
    use super::*;

    #[test]
    fn flag_spellings() {
        for on in ["1", "true", "yes", "on", "anything", " 1 ", "TRUE"] {
            std::env::set_var("DOCLING_TEST_FLAG_ON", on);
            assert!(flag("DOCLING_TEST_FLAG_ON"), "{on:?} should enable");
        }
        for off in ["", "0", "false", "no", "off", " OFF ", "No"] {
            std::env::set_var("DOCLING_TEST_FLAG_OFF", off);
            assert!(!flag("DOCLING_TEST_FLAG_OFF"), "{off:?} should disable");
        }
        assert!(!flag("DOCLING_TEST_FLAG_UNSET"));
    }

    #[test]
    fn nonempty_trims_and_drops_blank() {
        std::env::set_var("DOCLING_TEST_NONEMPTY", "  x  ");
        assert_eq!(nonempty("DOCLING_TEST_NONEMPTY").as_deref(), Some("x"));
        std::env::set_var("DOCLING_TEST_NONEMPTY_BLANK", "   ");
        assert_eq!(nonempty("DOCLING_TEST_NONEMPTY_BLANK"), None);
        assert_eq!(nonempty("DOCLING_TEST_NONEMPTY_UNSET"), None);
    }

    #[test]
    fn parse_trims_and_ignores_garbage() {
        std::env::set_var("DOCLING_TEST_PARSE", " 42 ");
        assert_eq!(parse::<usize>("DOCLING_TEST_PARSE"), Some(42));
        std::env::set_var("DOCLING_TEST_PARSE_BAD", "many");
        assert_eq!(parse::<usize>("DOCLING_TEST_PARSE_BAD"), None);
        assert_eq!(parse::<usize>("DOCLING_TEST_PARSE_UNSET"), None);
    }
}
