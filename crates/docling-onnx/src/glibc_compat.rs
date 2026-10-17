//! glibc ≥ 2.32 / 2.38 symbols the statically linked ONNX Runtime references,
//! defined here so the binaries keep their glibc 2.28 floor (#745).
//!
//! `ort`'s `download-binaries` links pyke's prebuilt `libonnxruntime.a`, which
//! is compiled against glibc 2.38+ headers: every `strtol`-family call in it is
//! the C23 `__isoc23_*` entry point glibc 2.38 introduced (its headers
//! redirect to them under `_GNU_SOURCE`), and libstdc++'s inline atomics read
//! `__libc_single_threaded` (glibc 2.32). The manylinux_2_28 wheel build links
//! against glibc 2.28, where none of them exist — and a *shared* object may
//! carry undefined symbols, so the link succeeds, auditwheel (which assumes an
//! unversioned undefined symbol comes from the interpreter) passes it, and the
//! wheel only fails at `import docling_rs` on any glibc < 2.38 with
//! `undefined symbol: __isoc23_strtol` (Debian 12, Ubuntu 22.04, RHEL 8/9, …;
//! `__libc_single_threaded` would be next on glibc < 2.32). Defining them in
//! this crate — which every crate that owns an `ort` session links — makes the
//! linker bind ONNX Runtime's references here instead of leaving them to the
//! runtime loader. The same holds for every other artifact linking the
//! prebuilt runtime on an older glibc (the npm addon, a source build of the
//! CLI); where glibc does have them (a 2.38+ build host) the definitions here
//! still win the link, which is fine: they are behaviorally identical.
//!
//! * `__isoc23_strto{l,ul,ll,ull}` and `__isoc23_strto{ll,ull}_l` (the six the
//!   archive references) are the C17 functions — present in every glibc —
//!   plus C23's one addition: with base 0 or 2, a `0b`/`0B` prefix followed by
//!   a binary digit introduces a base-2 number (C17 stops at the `b`). That
//!   case is converted by the C17 function in base 2 over the digits alone, so
//!   overflow (`ERANGE`, the saturated value) and the sign follow the C17 rules
//!   exactly; everything else is forwarded unchanged. Not merely forwarding
//!   keeps a build on a 2.38+ host (where these definitions interpose on
//!   glibc's for any library that imports them) byte-for-byte equivalent —
//!   the tests below check it against glibc's own `__isoc23_*` when present.
//! * `__libc_single_threaded` is permanently 0 ("may be multi-threaded"):
//!   libstdc++ only uses it to skip atomic reference-count updates when it is
//!   non-zero, so 0 is the always-correct conservative answer (ONNX Runtime
//!   runs thread pools anyway). It sits in writable memory (an atomic) like
//!   glibc's own `char`.
//!
//! Compiled only for linux-gnu builds that link ONNX Runtime statically: the
//! `load-dynamic` feature (#504) `dlopen`s a separately built library that
//! brings its own glibc requirements, and musl / macOS / Windows have no such
//! symbols to provide.

use std::ffi::{c_char, c_int, c_long, c_longlong, c_ulong, c_ulonglong, c_void};
use std::sync::atomic::AtomicI8;

/// glibc's `locale_t` (an opaque pointer).
type Locale = *mut c_void;

extern "C" {
    fn strtol(nptr: *const c_char, endptr: *mut *mut c_char, base: c_int) -> c_long;
    fn strtoul(nptr: *const c_char, endptr: *mut *mut c_char, base: c_int) -> c_ulong;
    fn strtoll(nptr: *const c_char, endptr: *mut *mut c_char, base: c_int) -> c_longlong;
    fn strtoull(nptr: *const c_char, endptr: *mut *mut c_char, base: c_int) -> c_ulonglong;
    fn strtoll_l(
        nptr: *const c_char,
        endptr: *mut *mut c_char,
        base: c_int,
        loc: Locale,
    ) -> c_longlong;
    fn strtoull_l(
        nptr: *const c_char,
        endptr: *mut *mut c_char,
        base: c_int,
        loc: Locale,
    ) -> c_ulonglong;
    fn isspace(c: c_int) -> c_int;
    fn isspace_l(c: c_int, loc: Locale) -> c_int;
}

/// See the module docs: 0 = "the process may be multi-threaded", which keeps
/// libstdc++ on its atomic paths.
#[no_mangle]
#[allow(non_upper_case_globals)]
static __libc_single_threaded: AtomicI8 = AtomicI8::new(0);

/// Where the binary digits start when `nptr` holds a C23 binary constant:
/// after leading white space (as `space` classifies it — the same locale
/// test glibc applies) and an optional sign, `0b`/`0B` followed by `0`/`1`.
/// Returns the sign and the first digit, or `None` for every other input.
///
/// # Safety
/// `nptr` must point to a NUL-terminated string.
unsafe fn binary_digits(
    nptr: *const c_char,
    space: &dyn Fn(c_int) -> bool,
) -> Option<(bool, *const u8)> {
    let mut p = nptr.cast::<u8>();
    // SAFETY (all reads): each read is at or before the string's NUL — the
    // loops stop at the first byte that is not white space / not the next
    // expected character, and NUL is neither.
    unsafe {
        while space(c_int::from(*p)) {
            p = p.add(1);
        }
        let negative = *p == b'-';
        if negative || *p == b'+' {
            p = p.add(1);
        }
        let prefixed = *p == b'0' && (*p.add(1) | 0x20) == b'b' && matches!(*p.add(2), b'0' | b'1');
        prefixed.then(|| (negative, p.add(2)))
    }
}

/// The C23 conversion: a binary constant (base 0 or 2) is converted by the C17
/// function `c17` in base 2 over the sign and the digits alone; anything else
/// is forwarded to `c17` as is.
///
/// # Safety
/// `nptr` must point to a NUL-terminated string; `endptr` must be null or
/// writable — the contract of the C function being implemented.
unsafe fn c23<T>(
    nptr: *const c_char,
    endptr: *mut *mut c_char,
    base: c_int,
    space: &dyn Fn(c_int) -> bool,
    c17: &dyn Fn(*const c_char, *mut *mut c_char, c_int) -> T,
) -> T {
    if base == 0 || base == 2 {
        // SAFETY: the caller's contract.
        if let Some((negative, digits)) = unsafe { binary_digits(nptr, space) } {
            let mut len = 0;
            // SAFETY: stops at the first non-binary byte, at the latest the NUL.
            while matches!(unsafe { *digits.add(len) }, b'0' | b'1') {
                len += 1;
            }
            // "-" + digits + NUL: C17 base 2 over it gives the C23 value,
            // saturation and errno; the whole digit run is the subject
            // sequence even on overflow, so the end is just past it.
            let mut subject = Vec::with_capacity(len + 2);
            if negative {
                subject.push(b'-');
            }
            // SAFETY: the `len` bytes just scanned.
            subject.extend_from_slice(unsafe { std::slice::from_raw_parts(digits, len) });
            subject.push(0);
            let value = c17(subject.as_ptr().cast(), std::ptr::null_mut(), 2);
            if !endptr.is_null() {
                // SAFETY: the caller's contract; `digits + len` is within the string.
                unsafe { *endptr = digits.add(len).cast_mut().cast() };
            }
            return value;
        }
    }
    c17(nptr, endptr, base)
}

fn space(c: c_int) -> bool {
    // SAFETY: `c` is an `unsigned char` value, as `isspace` requires.
    unsafe { isspace(c) != 0 }
}

/// # Safety
/// The C contract of `strtol`.
#[no_mangle]
unsafe extern "C" fn __isoc23_strtol(
    nptr: *const c_char,
    endptr: *mut *mut c_char,
    base: c_int,
) -> c_long {
    // SAFETY: forwarded caller contract.
    unsafe { c23(nptr, endptr, base, &space, &|s, e, b| strtol(s, e, b)) }
}

/// # Safety
/// The C contract of `strtoul`.
#[no_mangle]
unsafe extern "C" fn __isoc23_strtoul(
    nptr: *const c_char,
    endptr: *mut *mut c_char,
    base: c_int,
) -> c_ulong {
    // SAFETY: forwarded caller contract.
    unsafe { c23(nptr, endptr, base, &space, &|s, e, b| strtoul(s, e, b)) }
}

/// # Safety
/// The C contract of `strtoll`.
#[no_mangle]
unsafe extern "C" fn __isoc23_strtoll(
    nptr: *const c_char,
    endptr: *mut *mut c_char,
    base: c_int,
) -> c_longlong {
    // SAFETY: forwarded caller contract.
    unsafe { c23(nptr, endptr, base, &space, &|s, e, b| strtoll(s, e, b)) }
}

/// # Safety
/// The C contract of `strtoull`.
#[no_mangle]
unsafe extern "C" fn __isoc23_strtoull(
    nptr: *const c_char,
    endptr: *mut *mut c_char,
    base: c_int,
) -> c_ulonglong {
    // SAFETY: forwarded caller contract.
    unsafe { c23(nptr, endptr, base, &space, &|s, e, b| strtoull(s, e, b)) }
}

/// # Safety
/// The C contract of `strtoll_l` (`loc` a valid `locale_t`).
#[no_mangle]
unsafe extern "C" fn __isoc23_strtoll_l(
    nptr: *const c_char,
    endptr: *mut *mut c_char,
    base: c_int,
    loc: Locale,
) -> c_longlong {
    // SAFETY: forwarded caller contract.
    unsafe {
        c23(
            nptr,
            endptr,
            base,
            &|c| isspace_l(c, loc) != 0,
            &|s, e, b| strtoll_l(s, e, b, loc),
        )
    }
}

/// # Safety
/// The C contract of `strtoull_l` (`loc` a valid `locale_t`).
#[no_mangle]
unsafe extern "C" fn __isoc23_strtoull_l(
    nptr: *const c_char,
    endptr: *mut *mut c_char,
    base: c_int,
    loc: Locale,
) -> c_ulonglong {
    // SAFETY: forwarded caller contract.
    unsafe {
        c23(
            nptr,
            endptr,
            base,
            &|c| isspace_l(c, loc) != 0,
            &|s, e, b| strtoull_l(s, e, b, loc),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::sync::atomic::Ordering;

    extern "C" {
        fn __errno_location() -> *mut c_int;
        fn duplocale(loc: Locale) -> Locale;
        fn freelocale(loc: Locale);
    }

    /// glibc's `LC_GLOBAL_LOCALE`.
    const LC_GLOBAL_LOCALE: Locale = -1isize as Locale;

    /// (value, bytes consumed, errno) of one conversion.
    type Outcome<T> = (T, usize, c_int);

    fn run<T>(
        input: &str,
        base: c_int,
        f: impl Fn(*const c_char, *mut *mut c_char, c_int) -> T,
    ) -> Outcome<T> {
        let s = CString::new(input).unwrap();
        let mut end: *mut c_char = std::ptr::null_mut();
        // SAFETY: errno is thread-local; `f` gets a NUL-terminated string and
        // a writable end pointer.
        unsafe {
            *__errno_location() = 0;
            let value = f(s.as_ptr(), &mut end, base);
            let errno = *__errno_location();
            (value, end.offset_from(s.as_ptr()) as usize, errno)
        }
    }

    fn l(input: &str, base: c_int) -> Outcome<c_long> {
        run(input, base, |s, e, b| unsafe { __isoc23_strtol(s, e, b) })
    }

    #[test]
    fn binary_prefix_is_c23() {
        const ERANGE: c_int = 34;
        assert_eq!(l("0b101", 0), (5, 5, 0));
        assert_eq!(l("0B101", 2), (5, 5, 0));
        assert_eq!(l("  -0b11 tail", 0), (-3, 7, 0));
        assert_eq!(l("+0b1", 0), (1, 4, 0));
        // No binary digit after the prefix: C17's "0", ending at the `b`.
        assert_eq!(l("0b", 0), (0, 1, 0));
        assert_eq!(l("0b2", 2), (0, 1, 0));
        // Other bases never take the prefix (`0b1` is hex b1 in base 16).
        assert_eq!(l("0b1", 16), (0xb1, 3, 0));
        assert_eq!(l("0b1", 10), (0, 1, 0));
        assert_eq!(l("0b1", 8), (0, 1, 0));
        // Hex/octal prefixes and plain decimal are untouched.
        assert_eq!(l("0x1F", 0), (31, 4, 0));
        assert_eq!(l("017", 0), (15, 3, 0));
        assert_eq!(l("-42", 10), (-42, 3, 0));
        assert_eq!(l("junk", 0), (0, 0, 0));
        // Overflow saturates with ERANGE and consumes every digit.
        let max = format!("0b{}", "1".repeat(64));
        assert_eq!(l(&max, 0), (c_long::MAX, max.len(), ERANGE));
        let min = format!("-0b1{}", "0".repeat(63));
        assert_eq!(l(&min, 0), (c_long::MIN, min.len(), 0));
        let under = format!("-0b1{}", "0".repeat(64));
        assert_eq!(l(&under, 0), (c_long::MIN, under.len(), ERANGE));
        let ull = run(&max, 2, |s, e, b| unsafe { __isoc23_strtoull(s, e, b) });
        assert_eq!(ull, (c_ulonglong::MAX, max.len(), 0));
        // Unsigned conversions negate, as in C17.
        let neg = run("-0b1", 0, |s, e, b| unsafe { __isoc23_strtoul(s, e, b) });
        assert_eq!(neg, (c_ulong::MAX, 4, 0));
    }

    #[test]
    fn libc_single_threaded_is_conservative() {
        assert_eq!(__libc_single_threaded.load(Ordering::Relaxed), 0);
    }

    /// Inputs where C17 and C23 differ, and the edges around them.
    fn corpus() -> Vec<String> {
        let mut v: Vec<String> = [
            "0b101",
            "0B101",
            "-0b101",
            "+0b11",
            " \t\n0b1",
            "0b",
            "0B",
            "0b2",
            "0b102",
            "0bz",
            "00b1",
            "0b0b1",
            "0b1 tail",
            "-0b",
            "- 0b1",
            "0x1f",
            "0X1F",
            "0x",
            "017",
            "0",
            "",
            "  ",
            "-",
            "+",
            "101",
            "-1",
            "z",
            "Zz",
            "9223372036854775807",
            "9223372036854775808",
            "-9223372036854775808",
            "-9223372036854775809",
            "18446744073709551615",
            "18446744073709551616",
            "-18446744073709551616",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for n in [31, 32, 33, 63, 64, 65, 200] {
            v.push(format!("0b{}", "1".repeat(n)));
            v.push(format!("-0b1{}", "0".repeat(n)));
            v.push(format!("0b{}", "0".repeat(n)));
        }
        v
    }

    /// Our definitions must reproduce glibc's own C23 functions exactly
    /// (value, end pointer, errno) — checked wherever the host glibc has them
    /// (2.38+); older hosts run only `binary_prefix_is_c23`.
    #[test]
    fn matches_glibc_isoc23_when_available() {
        type Plain<T> = unsafe extern "C" fn(*const c_char, *mut *mut c_char, c_int) -> T;
        type WithLocale<T> =
            unsafe extern "C" fn(*const c_char, *mut *mut c_char, c_int, Locale) -> T;

        // SAFETY: loading the C library this process already has mapped;
        // `dlsym` on its handle finds glibc's definition, not ours.
        let libc = match unsafe { libloading::Library::new("libc.so.6") } {
            Ok(lib) => lib,
            Err(_) => return,
        };
        // SAFETY: the symbols' C signatures are the declared ones.
        let (Ok(g_l), Ok(g_ul), Ok(g_ll), Ok(g_ull), Ok(g_ll_l), Ok(g_ull_l)) = (unsafe {
            (
                libc.get::<Plain<c_long>>(b"__isoc23_strtol\0"),
                libc.get::<Plain<c_ulong>>(b"__isoc23_strtoul\0"),
                libc.get::<Plain<c_longlong>>(b"__isoc23_strtoll\0"),
                libc.get::<Plain<c_ulonglong>>(b"__isoc23_strtoull\0"),
                libc.get::<WithLocale<c_longlong>>(b"__isoc23_strtoll_l\0"),
                libc.get::<WithLocale<c_ulonglong>>(b"__isoc23_strtoull_l\0"),
            )
        }) else {
            eprintln!("glibc < 2.38: no __isoc23_* to compare against");
            return;
        };
        // SAFETY: a copy of the global locale, freed below.
        let loc = unsafe { duplocale(LC_GLOBAL_LOCALE) };
        assert!(!loc.is_null());

        let mut checked = 0;
        for input in corpus() {
            for base in [0, 2, 8, 10, 16, 36] {
                macro_rules! same {
                    ($ours:expr, $glibc:expr) => {{
                        let ours = run(&input, base, $ours);
                        let theirs = run(&input, base, $glibc);
                        assert_eq!(ours, theirs, "{input:?} base {base}");
                        checked += 1;
                    }};
                }
                unsafe {
                    same!(|s, e, b| __isoc23_strtol(s, e, b), |s, e, b| g_l(s, e, b));
                    same!(|s, e, b| __isoc23_strtoul(s, e, b), |s, e, b| g_ul(s, e, b));
                    same!(|s, e, b| __isoc23_strtoll(s, e, b), |s, e, b| g_ll(s, e, b));
                    same!(|s, e, b| __isoc23_strtoull(s, e, b), |s, e, b| g_ull(
                        s, e, b
                    ));
                    same!(
                        |s, e, b| __isoc23_strtoll_l(s, e, b, loc),
                        |s, e, b| g_ll_l(s, e, b, loc)
                    );
                    same!(|s, e, b| __isoc23_strtoull_l(s, e, b, loc), |s, e, b| {
                        g_ull_l(s, e, b, loc)
                    });
                }
            }
        }
        // SAFETY: allocated by `duplocale` above, no longer used.
        unsafe { freelocale(loc) };
        assert!(checked > 1000);
    }
}
