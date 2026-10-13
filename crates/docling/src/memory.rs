//! Allocator housekeeping for a process that converts many documents — a
//! server, an agent tool, a batch worker (#263, #669).
//!
//! A conversion churns hundreds of MB of short-lived buffers (page bitmaps,
//! ONNX Runtime tensors) across many threads. glibc's allocator keeps what
//! they free: measured on the MCP server, 8 MB was live after a 9-page PDF
//! while RSS sat at 0.5–1.2 GB — all of it free memory held in 21 per-thread
//! arenas, growing call over call. Two causes, two calls:
//!
//! - [`pin_mmap_threshold`], once at startup: glibc raises its mmap
//!   threshold (up to 32 MiB) every time a large mapped block is freed, after
//!   which blocks of that size are carved from the arenas, where they
//!   fragment and stay resident. A fixed threshold turns the adjustment off,
//!   so large buffers are mapped and unmapped again.
//! - [`release_free_memory`], after each conversion: hands the free pages
//!   still inside the arenas back to the OS.
//!
//! Both are glibc-only (Linux with the GNU C library) and do nothing
//! elsewhere — musl and the macOS/Windows allocators lack the pattern and
//! the calls.

/// The fixed mmap threshold [`pin_mmap_threshold`] sets. Any fixed value
/// stops the growth; measured on a warm MCP server (9-page paper, 4 vCPU,
/// ONNX arena off) RSS held at ~475 MB with 1, 4 or 16 MiB against 500 →
/// 750 MB without, and from 4 MiB up the conversions were as fast as
/// glibc's default — at 128 KiB every ONNX intermediate tensor was mapped
/// and conversions ran ~8% slower.
pub const MMAP_THRESHOLD: usize = 4 << 20;

/// Pin glibc's mmap threshold at [`MMAP_THRESHOLD`] for the rest of the
/// process, unless the operator chose one (`GLIBC_TUNABLES` naming
/// `glibc.malloc.mmap_threshold`, or `MALLOC_MMAP_THRESHOLD_`). Call once,
/// early, in a long-lived process; a one-shot conversion gains nothing.
pub fn pin_mmap_threshold() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        let tunables = std::env::var("GLIBC_TUNABLES").unwrap_or_default();
        if tunables.contains("glibc.malloc.mmap_threshold")
            || std::env::var_os("MALLOC_MMAP_THRESHOLD_").is_some()
        {
            return;
        }
        // SAFETY: `mallopt` only changes the allocator's parameters; the
        // value is a valid threshold (glibc caps it at 32 MiB on 64-bit).
        unsafe {
            libc::mallopt(libc::M_MMAP_THRESHOLD, MMAP_THRESHOLD as libc::c_int);
        }
    }
}

/// Return the free memory glibc still holds in its arenas to the OS —
/// after a conversion, once its buffers are dropped.
pub fn release_free_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: `malloc_trim` only releases free memory; it touches no live
    // allocation.
    unsafe {
        libc::malloc_trim(0);
    }
}
