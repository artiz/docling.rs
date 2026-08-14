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

/// The CPU budget thread-pool sizing should derive from: host parallelism
/// clamped by the container's cgroup CPU quota (#262).
/// `available_parallelism` is quota-aware on common setups, but container
/// runtimes exist where it still reports the host cores (docling.rs#262's
/// 8 threads under a 4-CPU limit), so the quota files are read directly as an
/// extra clamp — a limited container must never size pools past its throttle
/// ceiling. On non-Linux (and wasm) the quota reads simply fail and the host
/// count stands.
pub fn cpu_budget() -> usize {
    let host = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    match cgroup_cpu_quota() {
        Some(q) => host.min(q).max(1),
        None => host,
    }
}

/// CPUs allowed by the cgroup CPU quota, rounded up (a 2.5-CPU limit gets 3
/// threads); `None` when unlimited or undeterminable. Reads cgroup v2
/// (`cpu.max`) and cgroup v1 (`cpu.cfs_quota_us` / `cpu.cfs_period_us`).
fn cgroup_cpu_quota() -> Option<usize> {
    if let Ok(s) = std::fs::read_to_string("/sys/fs/cgroup/cpu.max") {
        return parse_cpu_max(&s);
    }
    for dir in ["/sys/fs/cgroup/cpu", "/sys/fs/cgroup/cpu,cpuacct"] {
        if let (Ok(quota), Ok(period)) = (
            std::fs::read_to_string(format!("{dir}/cpu.cfs_quota_us")),
            std::fs::read_to_string(format!("{dir}/cpu.cfs_period_us")),
        ) {
            return parse_cfs(&quota, &period);
        }
    }
    None
}

/// cgroup v2 `cpu.max` ("max 100000" = unlimited, "400000 100000" = 4 CPUs).
fn parse_cpu_max(s: &str) -> Option<usize> {
    let mut it = s.split_whitespace();
    let quota = it.next()?;
    if quota == "max" {
        return None;
    }
    let quota: u64 = quota.parse().ok()?;
    let period: u64 = it.next()?.parse().ok()?;
    if period == 0 || quota == 0 {
        return None;
    }
    Some(quota.div_ceil(period) as usize)
}

/// cgroup v1 CFS quota/period (quota -1 = unlimited).
fn parse_cfs(quota: &str, period: &str) -> Option<usize> {
    let quota: i64 = quota.trim().parse().ok()?;
    if quota <= 0 {
        return None;
    }
    let period: i64 = period.trim().parse().ok()?;
    if period <= 0 {
        return None;
    }
    Some((quota as u64).div_ceil(period as u64) as usize)
}

/// The container's memory limit in MB from the cgroup files (v2 `memory.max`,
/// v1 `memory.limit_in_bytes`), `None` when unlimited — both spell "no limit"
/// as either the literal `max` or an enormous sentinel (>= 2^60 bytes).
pub fn cgroup_memory_limit_mb() -> Option<u64> {
    for path in [
        "/sys/fs/cgroup/memory.max",
        "/sys/fs/cgroup/memory/memory.limit_in_bytes",
    ] {
        if let Ok(s) = std::fs::read_to_string(path) {
            let s = s.trim();
            if s == "max" {
                return None;
            }
            let bytes: u64 = s.parse().ok()?;
            if bytes >= 1 << 60 {
                return None;
            }
            return Some(bytes / (1024 * 1024));
        }
    }
    None
}

/// This process's resident set size in MB (`/proc/self/status` `VmRSS`);
/// `None` off Linux. The number admission control (#263) compares against the
/// memory ceiling.
pub fn rss_mb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
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
    fn cpu_quota_parsers_cover_both_cgroup_versions() {
        // v2: unlimited, exact, and fractional (rounds up).
        assert_eq!(super::parse_cpu_max("max 100000\n"), None);
        assert_eq!(super::parse_cpu_max("400000 100000"), Some(4));
        assert_eq!(super::parse_cpu_max("250000 100000"), Some(3));
        assert_eq!(super::parse_cpu_max("garbage"), None);
        // v1: -1 = unlimited; fractional rounds up.
        assert_eq!(super::parse_cfs("-1\n", "100000\n"), None);
        assert_eq!(super::parse_cfs("400000", "100000"), Some(4));
        assert_eq!(super::parse_cfs("150000", "100000"), Some(2));
        assert_eq!(super::parse_cfs("x", "100000"), None);
    }

    #[test]
    fn rss_reads_on_linux() {
        // A running test process certainly has a nonzero RSS on Linux.
        #[cfg(target_os = "linux")]
        assert!(super::rss_mb().unwrap() > 0);
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
