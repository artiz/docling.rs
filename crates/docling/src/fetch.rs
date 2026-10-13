//! Loading a document from somewhere other than a file: a URL (#668), a
//! `data:` URI, a reader such as stdin.
//!
//! The URL fetch is the one docling-serve has run since its URL inputs — the
//! SSRF guard ([`check_outbound_url`]), the size and time bounds, the
//! name-and-format resolution from `Content-Disposition`, the URL path and
//! `Content-Type` — lifted here so the CLI and the library share it. The
//! policy is explicit ([`FetchPolicy`]): a server refuses redirects and keeps
//! the private-address block on, a CLI run on a developer's laptop follows a
//! few redirects and may be pointed at `localhost`.

use std::net::IpAddr;

use crate::error::ConversionError;
use crate::source::SourceDocument;

/// Whether a resolved IP points back at the local host or private
/// infrastructure — the SSRF block-list behind every URL docling fetches on
/// someone else's behalf (docling-serve's URL inputs and `vlm_endpoint`, the
/// `<img src>` fetch, the CLI's URL sources). Without it a request for
/// `http://169.254.169.254/…` or `http://127.0.0.1:…` reaches cloud metadata
/// and internal services from the converter's network position.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                // Carrier-grade NAT 100.64.0.0/10.
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                // Unique-local fc00::/7 and link-local fe80::/10.
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                // IPv4-mapped (::ffff:a.b.c.d): re-check the embedded v4.
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| is_blocked_ip(IpAddr::V4(v4)))
        }
    }
}

/// Operator opt-out of the private-address block for local development
/// (`DOCLING_RS_ALLOW_PRIVATE_IP_FETCH=1`): a URL like
/// `http://localhost:8080/doc.pdf` can then be fetched. Off by default —
/// leave it unset in production.
pub fn allow_private_ip_fetch() -> bool {
    docling_core::env::flag("DOCLING_RS_ALLOW_PRIVATE_IP_FETCH")
}

/// SSRF pre-check for a URL supplied by whoever asked for the conversion:
/// `http(s)` scheme only, then resolve the host and refuse one that maps to
/// a private / loopback / link-local address — unless `allow_private` (or
/// the `DOCLING_RS_ALLOW_PRIVATE_IP_FETCH` opt-out) lifts the block. A
/// fetch that follows redirects must re-run this on every hop: a public URL
/// can 30x-bounce onto an internal target. Best effort — a DNS-rebinding
/// race between this resolution and the connect remains possible; the
/// deployment-level control is to keep URL fetch off on untrusted networks.
/// Blocking (DNS).
#[cfg(feature = "fetch-url")]
pub fn check_outbound_url(url: &str, allow_private: bool) -> Result<(), String> {
    use std::net::ToSocketAddrs;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(format!("unsupported URL scheme in '{url}'"));
    }
    let parsed = url::Url::parse(url).map_err(|e| format!("bad URL '{url}': {e}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| format!("no host in URL '{url}'"))?;
    let port = parsed.port_or_known_default().unwrap_or(80);
    let mut resolved = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {host}: {e}"))?
        .peekable();
    if resolved.peek().is_none() {
        return Err(format!("cannot resolve {host}"));
    }
    if !allow_private && !allow_private_ip_fetch() {
        for addr in resolved {
            if is_blocked_ip(addr.ip()) {
                return Err(format!(
                    "refusing to fetch {url}: resolves to a private/loopback address \
                     (set DOCLING_RS_ALLOW_PRIVATE_IP_FETCH=1 for local development)"
                ));
            }
        }
    }
    Ok(())
}

/// Largest URL-fetch response accepted by default (256 MiB): `read_to_end`
/// on a response is otherwise unbounded, and a hostile URL streaming an
/// endless body would exhaust memory. `DOCLING_RS_MAX_FETCH_BYTES`
/// overrides it for every policy built with [`FetchPolicy::default`].
pub const DEFAULT_MAX_FETCH_BYTES: u64 = 256 * 1024 * 1024;

/// The `Accept` header a document fetch sends: a server that negotiates
/// (a wiki, a docs site, an API that answers in Markdown or HTML) is asked
/// for Markdown first, HTML next, and anything it has otherwise — a PDF or
/// a DOCX link answers with its own type regardless.
pub const ACCEPT_DOCUMENT: &str = "text/markdown, text/html;q=0.9, */*;q=0.1";

/// How a URL is fetched — bounds and the SSRF stance.
#[derive(Debug, Clone)]
pub struct FetchPolicy {
    /// Largest response body accepted; a longer one is an error, not
    /// truncated. Default: `DOCLING_RS_MAX_FETCH_BYTES` or
    /// [`DEFAULT_MAX_FETCH_BYTES`].
    pub max_bytes: u64,
    /// Redirect hops followed, each re-checked against the SSRF guard. A
    /// server should keep it at 0 (docling-serve refuses redirects outright);
    /// the CLI follows a few. Default 0.
    pub max_redirects: u32,
    /// Connect timeout. Default 10 s.
    pub connect_timeout: std::time::Duration,
    /// Whole-transfer timeout. Default 300 s.
    pub timeout: std::time::Duration,
    /// Lift the private / loopback address block (the per-call form of
    /// `DOCLING_RS_ALLOW_PRIVATE_IP_FETCH`). Default `false`.
    pub allow_private: bool,
    /// Extra request headers sent verbatim (docling's `HttpSource.headers`,
    /// #139 — auth tokens for protected origins). `Accept` defaults to
    /// [`ACCEPT_DOCUMENT`] unless one is given here.
    pub headers: Vec<(String, String)>,
}

impl Default for FetchPolicy {
    fn default() -> Self {
        Self {
            max_bytes: docling_core::env::parse("DOCLING_RS_MAX_FETCH_BYTES")
                .unwrap_or(DEFAULT_MAX_FETCH_BYTES),
            max_redirects: 0,
            connect_timeout: std::time::Duration::from_secs(10),
            timeout: std::time::Duration::from_secs(300),
            allow_private: false,
            headers: Vec::new(),
        }
    }
}

/// What a fetch answered with, before it becomes a document: the final
/// URL, the body, and the two headers the name and format come from.
#[cfg(feature = "fetch-url")]
#[derive(Debug)]
pub struct Fetched {
    /// The URL the body came from — the last hop when redirects were followed.
    pub url: String,
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
    pub content_disposition: Option<String>,
}

/// Fetch `url` under `policy` (blocking). Every hop passes
/// [`check_outbound_url`]; a redirect past `policy.max_redirects` is an
/// error, as is a body longer than `policy.max_bytes`.
#[cfg(feature = "fetch-url")]
pub fn fetch(url: &str, policy: &FetchPolicy) -> Result<Fetched, ConversionError> {
    use std::io::Read;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .timeout_connect(Some(policy.connect_timeout))
        .timeout_global(Some(policy.timeout))
        .build()
        .into();
    let mut target = url.to_string();
    let mut hops = 0;
    loop {
        check_outbound_url(&target, policy.allow_private).map_err(ConversionError::Fetch)?;
        let mut request = agent.get(&target);
        if !policy
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("accept"))
        {
            request = request.header("Accept", ACCEPT_DOCUMENT);
        }
        for (k, v) in &policy.headers {
            request = request.header(k, v);
        }
        let mut response = request
            .call()
            .map_err(|e| ConversionError::Fetch(format!("fetching {target}: {e}")))?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    ConversionError::Fetch(format!(
                        "fetching {target}: redirect without a Location header"
                    ))
                })?;
            hops += 1;
            if hops > policy.max_redirects {
                return Err(ConversionError::Fetch(format!(
                    "fetching {url}: too many redirects (more than {}; last to {location})",
                    policy.max_redirects
                )));
            }
            target = url::Url::parse(&target)
                .ok()
                .and_then(|base| base.join(location).ok())
                .map(String::from)
                .ok_or_else(|| {
                    ConversionError::Fetch(format!("fetching {target}: bad redirect to {location}"))
                })?;
            continue;
        }
        if !response.status().is_success() {
            return Err(ConversionError::Fetch(format!(
                "fetching {target}: HTTP {}",
                response.status()
            )));
        }
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let content_type = header("content-type");
        let content_disposition = header("content-disposition");
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(policy.max_bytes + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| ConversionError::Fetch(format!("reading {target}: {e}")))?;
        if bytes.len() as u64 > policy.max_bytes {
            return Err(ConversionError::Fetch(format!(
                "response from {target} exceeds {} bytes",
                policy.max_bytes
            )));
        }
        return Ok(Fetched {
            url: target,
            bytes,
            content_type,
            content_disposition,
        });
    }
}

/// The file name a fetch should be known by, in docling's order: the
/// `Content-Disposition` `filename` (its RFC 5987 `filename*` form first),
/// then the URL path's last segment, then `document`.
#[cfg(feature = "fetch-url")]
pub fn fetched_name(url: &str, content_disposition: Option<&str>) -> String {
    content_disposition
        .and_then(disposition_filename)
        .or_else(|| url_file_name(url))
        .unwrap_or_else(|| "document".to_string())
}

/// The last path segment of `url` (query and fragment stripped,
/// percent-decoded), or `None` for an origin / a trailing slash.
#[cfg(feature = "fetch-url")]
pub fn url_file_name(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let last = parsed.path_segments()?.next_back()?;
    if last.is_empty() {
        return None;
    }
    let decoded = percent_decode(last);
    let name = String::from_utf8_lossy(&decoded).into_owned();
    // A decoded segment that carries a path separator is not a file name.
    if name.contains(['/', '\\']) || name == "." || name == ".." {
        return None;
    }
    Some(name)
}

/// The local path a `file:` URL names (`file:///home/me/a.pdf`,
/// `file:a.pdf` is not one — RFC 8089 wants the authority slashes).
#[cfg(feature = "fetch-url")]
pub fn file_url_path(url: &str) -> Result<std::path::PathBuf, String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("bad URL '{url}': {e}"))?;
    if parsed.scheme() != "file" {
        return Err(format!("not a file: URL: '{url}'"));
    }
    parsed
        .to_file_path()
        .map_err(|()| format!("'{url}' is not a local file path"))
}

/// `filename` from a `Content-Disposition` value: `filename*=UTF-8''a%20b.pdf`
/// wins over `filename="a b.pdf"`, quotes and any directory part dropped.
pub fn disposition_filename(value: &str) -> Option<String> {
    let mut plain = None;
    let mut extended = None;
    for part in value.split(';').skip(1) {
        let Some((key, raw)) = part.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let raw = raw.trim();
        if key == "filename*" {
            // RFC 5987: charset'language'percent-encoded.
            let encoded = raw.rsplit('\'').next().unwrap_or(raw);
            let decoded = String::from_utf8_lossy(&percent_decode(encoded)).into_owned();
            extended = Some(decoded);
        } else if key == "filename" {
            plain = Some(raw.trim_matches('"').to_string());
        }
    }
    extended
        .or(plain)
        .map(|n| {
            n.rsplit(['/', '\\'])
                .next()
                .unwrap_or(&n)
                .trim()
                .to_string()
        })
        .filter(|n| !n.is_empty() && n != "." && n != "..")
}

/// A parsed `data:` URI (RFC 2397): the payload decoded, its media type
/// and the optional `name=` parameter kept for the caller to type and name
/// the document with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataUrl {
    pub media_type: Option<String>,
    pub name: Option<String>,
    pub bytes: Vec<u8>,
}

/// Parse a `data:[<media type>][;name=<file name>][;base64],<payload>` URI.
pub fn parse_data_url(url: &str) -> Result<DataUrl, ConversionError> {
    let rest = strip_data_scheme(url)
        .ok_or_else(|| ConversionError::Fetch(format!("not a data: URL: '{url}'")))?;
    let (meta, payload) = rest.split_once(',').ok_or_else(|| {
        ConversionError::Fetch("malformed data: URL: no ',' before the payload".to_string())
    })?;
    let mut params = meta.split(';');
    let media_type = params.next().filter(|m| !m.is_empty()).map(str::to_string);
    let mut base64 = false;
    let mut name = None;
    for p in params {
        if p.eq_ignore_ascii_case("base64") {
            base64 = true;
        } else if let Some(v) = p.strip_prefix("name=") {
            name = Some(String::from_utf8_lossy(&percent_decode(v.trim_matches('"'))).into_owned());
        }
    }
    let bytes = if base64 {
        docling_core::base64::decode(payload.trim()).ok_or_else(|| {
            ConversionError::Fetch("malformed data: URL: invalid base64 payload".to_string())
        })?
    } else {
        percent_decode(payload)
    };
    Ok(DataUrl {
        media_type,
        name,
        bytes,
    })
}

/// Decode a `data:` URI into a document: the payload is the bytes, the
/// media type names the format ([`crate::InputFormat::from_mime`]); without
/// one the bytes are sniffed. A `name=` parameter, when present, is the
/// document's name; a `charset=` other than UTF-8
/// (`data:text/csv;charset=shift_jis,…`) is the text encoding the
/// converter decodes with ([`SourceDocument::with_encoding`]).
pub fn from_data_url(url: &str) -> Result<SourceDocument, ConversionError> {
    let data = parse_data_url(url)?;
    let source = SourceDocument::from_named_bytes(
        data.name.as_deref().unwrap_or("document"),
        data.bytes,
        data.media_type.as_deref(),
    )?;
    Ok(match data_url_charset(url) {
        Some(label) if !label.eq_ignore_ascii_case("utf-8") => source.with_encoding(Some(label)),
        _ => source,
    })
}

/// `url` after its `data:` scheme — matched without regard to case, as URI
/// schemes are (RFC 3986 §3.1: `DATA:` is the same scheme).
fn strip_data_scheme(url: &str) -> Option<&str> {
    url.get(..5)
        .filter(|scheme| scheme.eq_ignore_ascii_case("data:"))
        .map(|_| &url[5..])
}

/// The `charset=` parameter of a `data:` URI's media type, if any.
fn data_url_charset(url: &str) -> Option<String> {
    let meta = strip_data_scheme(url)?.split_once(',')?.0;
    meta.split(';').skip(1).find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| v.trim().trim_matches('"').to_string())
    })
}

/// Percent-decoding (`%41` → `A`); a `%` not followed by two hex digits is
/// kept as is.
pub(crate) fn percent_decode(s: &str) -> Vec<u8> {
    fn hex(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::InputFormat;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn blocks_internal_targets() {
        // Loopback, private ranges, link-local (incl. cloud metadata),
        // unspecified, CGNAT, and the IPv4-mapped IPv6 forms must all be
        // refused as SSRF targets.
        for s in [
            "127.0.0.1",
            "127.5.5.5",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254", // AWS/GCP metadata
            "0.0.0.0",
            "100.64.0.1", // carrier-grade NAT
            "::1",
            "fe80::1",          // link-local
            "fc00::1",          // unique-local
            "::ffff:127.0.0.1", // IPv4-mapped loopback
            "::ffff:169.254.169.254",
        ] {
            assert!(is_blocked_ip(ip(s)), "{s} should be blocked");
        }
    }

    #[test]
    fn allows_public_targets() {
        for s in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "2606:4700:4700::1111",
        ] {
            assert!(!is_blocked_ip(ip(s)), "{s} should be allowed");
        }
    }

    #[test]
    fn disposition_filename_prefers_the_extended_form() {
        assert_eq!(
            disposition_filename(
                "attachment; filename=\"fallback.pdf\"; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf"
            )
            .as_deref(),
            Some("résumé.pdf")
        );
        assert_eq!(
            disposition_filename("inline; filename=\"dir/report.docx\"").as_deref(),
            Some("report.docx")
        );
        assert_eq!(disposition_filename("attachment"), None);
        assert_eq!(disposition_filename("attachment; filename=\"\""), None);
    }

    #[cfg(feature = "fetch-url")]
    #[test]
    fn fetched_name_falls_back_to_the_url_path() {
        assert_eq!(
            fetched_name("https://h.example/a/b/Report%20Q1.pdf?x=1#top", None),
            "Report Q1.pdf"
        );
        assert_eq!(fetched_name("https://h.example/", None), "document");
        assert_eq!(fetched_name("https://h.example", None), "document");
        assert_eq!(
            fetched_name("https://h.example/x", Some("attachment; filename=y.md")),
            "y.md"
        );
    }

    #[cfg(feature = "fetch-url")]
    #[test]
    fn file_url_resolves_to_a_local_path() {
        assert_eq!(
            file_url_path("file:///tmp/a%20b.pdf").unwrap(),
            std::path::PathBuf::from("/tmp/a b.pdf")
        );
        assert!(file_url_path("file://host.example/x.pdf").is_err());
        assert!(file_url_path("https://h.example/x.pdf").is_err());
    }

    #[test]
    fn data_url_decodes_base64_and_percent_forms() {
        let doc = from_data_url("data:text/html;base64,PGgxPkhpPC9oMT4=").unwrap();
        assert_eq!(doc.format, InputFormat::Html);
        assert_eq!(doc.bytes, b"<h1>Hi</h1>");
        assert_eq!(doc.name, "document");
        let doc = from_data_url("data:text/markdown;name=notes.md,%23%20Title").unwrap();
        assert_eq!(doc.format, InputFormat::Md);
        assert_eq!(doc.bytes, b"# Title");
        assert_eq!(doc.name, "notes");
        // No media type: the bytes decide.
        let doc = from_data_url("data:;base64,JVBERi0xLjQK").unwrap();
        assert_eq!(doc.format, InputFormat::Pdf);
        assert!(from_data_url("data:text/plain").is_err());
        assert!(from_data_url("data:;base64,@@@").is_err());
        assert!(from_data_url("data:application/octet-stream,zzz").is_err());
        // A non-UTF-8 charset is the text encoding; base64 may wrap.
        let doc = from_data_url("data:text/csv;charset=shift_jis,a,b").unwrap();
        assert_eq!(doc.encoding.as_deref(), Some("shift_jis"));
        let doc = from_data_url("data:text/csv;charset=\"UTF-8\",a,b").unwrap();
        assert_eq!(doc.encoding, None);
        let doc = from_data_url("data:text/markdown;base64,IyBUaXRs\nZQ==").unwrap();
        assert_eq!(doc.bytes, b"# Title");
        // The scheme is case-insensitive.
        let doc = from_data_url("DATA:text/html,%3Cp%3Ex%3C/p%3E").unwrap();
        assert_eq!(doc.format, InputFormat::Html);
    }
}
