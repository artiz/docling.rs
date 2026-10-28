//! Lightweight, env-gated per-stage timing for profiling the PDF pipeline.
//!
//! Set `DOCLING_RS_TIMING=1` to accumulate wall-clock time per named stage
//! across all pages and print a sorted breakdown at process exit. Zero cost
//! when the env var is unset (the `Instant` is still taken but nothing is
//! recorded — call sites guard the hot path themselves where it matters).
//!
//! The totals are process-wide and cumulative ([`report`] prints them after
//! every conversion). An embedder profiling one document at a time reads
//! them with [`snapshot`] and clears them with [`reset`] (#897) instead of
//! parsing stderr.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| docling_core::env::flag("DOCLING_RS_TIMING"))
}

fn store() -> &'static Mutex<BTreeMap<&'static str, (u128, u64)>> {
    static S: OnceLock<Mutex<BTreeMap<&'static str, (u128, u64)>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Record `elapsed` nanoseconds against `stage`. No-op unless timing is enabled.
pub fn record(stage: &'static str, nanos: u128) {
    if !enabled() {
        return;
    }
    let mut g = store().lock().unwrap();
    let e = g.entry(stage).or_insert((0, 0));
    e.0 += nanos;
    e.1 += 1;
}

/// Time a closure and attribute its wall-clock to `stage`.
pub fn timed<T>(stage: &'static str, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }
    let t = Instant::now();
    let out = f();
    record(stage, t.elapsed().as_nanos());
    out
}

/// Print the accumulated breakdown (descending by total time). Call once at the
/// end of a conversion. No-op unless timing is enabled.
pub fn report() {
    if !enabled() {
        return;
    }
    let g = store().lock().unwrap();
    let mut rows: Vec<_> = g.iter().map(|(k, v)| (*k, v.0, v.1)).collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.1));
    let total: u128 = rows.iter().map(|r| r.1).sum();
    eprintln!("=== DOCLING_RS timing (per stage, wall-clock) ===");
    for (stage, nanos, calls) in &rows {
        let ms = *nanos as f64 / 1e6;
        let pct = if total > 0 {
            *nanos as f64 / total as f64 * 100.0
        } else {
            0.0
        };
        eprintln!("  {stage:<22} {ms:>9.1} ms  {pct:>5.1}%  ({calls} calls)");
    }
    eprintln!("  {:<22} {:>9.1} ms", "TOTAL (summed)", total as f64 / 1e6);
}

/// One stage's accumulated time, as [`snapshot`] returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageTiming {
    /// The stage name (`layout.predict`, `render.page`, `tableformer`, …).
    pub stage: &'static str,
    /// Wall-clock time summed over every call (stages on parallel threads
    /// overlap, so the totals can exceed the conversion's wall time).
    pub total: std::time::Duration,
    /// How many times the stage ran.
    pub calls: u64,
}

/// The stages accumulated since the process started or the last [`reset`],
/// longest first. Empty unless `DOCLING_RS_TIMING` is set.
pub fn snapshot() -> Vec<StageTiming> {
    let g = store().lock().unwrap_or_else(|p| p.into_inner());
    let mut rows: Vec<StageTiming> = g
        .iter()
        .map(|(stage, (nanos, calls))| StageTiming {
            stage,
            total: std::time::Duration::from_nanos(u64::try_from(*nanos).unwrap_or(u64::MAX)),
            calls: *calls,
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.total));
    rows
}

/// Forget the accumulated stages — e.g. between documents, so the next
/// [`snapshot`] (and [`report`]) covers one conversion.
pub fn reset() {
    store().lock().unwrap_or_else(|p| p.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_and_reset() {
        // Recording is gated on DOCLING_RS_TIMING, read once per process:
        // the shape is checked either way, the values when it is on.
        reset();
        record("test.stage", 2_000_000);
        record("test.stage", 1_000_000);
        let rows = snapshot();
        if enabled() {
            let row = rows.iter().find(|r| r.stage == "test.stage").unwrap();
            assert_eq!((row.total.as_nanos(), row.calls), (3_000_000, 2));
        } else {
            assert!(rows.is_empty());
        }
        reset();
        assert!(snapshot().iter().all(|r| r.stage != "test.stage"));
    }
}
