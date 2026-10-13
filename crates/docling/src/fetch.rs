//! Bounded HTTP(S) download of a document named by URL — the outbound fetch a
//! surface that takes URL inputs performs on the caller's behalf (the MCP
//! server's `convert_to_markdown(uri)`, #669).
//!
//! Fetching a caller-supplied URL from a server is an SSRF surface: a URL
//! that resolves to `127.0.0.1`, a private range or the cloud metadata
//! address (`169.254.169.254`) would let the caller reach what only the
//! server can. [`check_url`] refuses those before connecting, and
//! [`fetch`] runs it on **every redirect hop** — redirects are followed by
//! hand, so a public URL cannot 30x-bounce the request onto an internal
//! address. The response is capped in size and the request in time, so a
//! hostile or slow URL cannot exhaust memory or pin a worker.
//! `DOCLING_RS_ALLOW_PRIVATE_IP_FETCH=1` lifts the address check for local
//! development, the same escape hatch docling-serve and the image resolver
//! honor. A DNS-rebinding race between the check and the connect remains
//! theoretically possible; the deployment-level control is to leave URL
//! fetching off where the network is not trusted.

use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;

use crate::{InputFormat, SourceDocument};

/// The default response-size cap: 256 MiB, docling-serve's URL-fetch limit.
pub const DEFAULT_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// How far a [`fetch`] may go.
#[derive(Clone, Debug)]
pub struct FetchLimits {
    /// Largest response body accepted, in bytes.
    pub max_bytes: u64,
    /// Time allowed to establish a connection.
    pub connect_timeout: Duration,
    /// Time allowed for the whole request, every redirect hop included.
    pub timeout: Duration,
    /// Redirect hops followed (each one re-checked) before giving up.
    pub max_redirects: usize,
}

impl Default for FetchLimits {
    /// 256 MiB, 10 s to connect, 300 s overall (a large PDF over a slow
    /// link), 5 redirects.
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_BYTES,
            connect_timeout: Duration::from_secs(10),
            timeout: Duration::from_secs(300),
            max_redirects: 5,
        }
    }
}

impl FetchLimits {
    /// The defaults, with the size cap from `DOCLING_RS_MAX_FETCH_BYTES` when
    /// set — the variable docling-serve's URL fetch reads.
    pub fn from_env() -> Self {
        let mut limits = Self::default();
        if let Some(max) = docling_core::env::parse::<u64>("DOCLING_RS_MAX_FETCH_BYTES") {
            limits.max_bytes = max;
        }
        limits
    }
}

/// Why a [`fetch`] failed; the message names the URL and what to change.
#[derive(Debug)]
pub struct FetchError(String);

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FetchError {}

/// A downloaded response.
#[derive(Clone, Debug)]
pub struct Fetched {
    /// The URL the body came from — the last redirect hop's.
    pub url: String,
    /// The response's `Content-Type`, if it sent one.
    pub content_type: Option<String>,
    /// The file name from `Content-Disposition`, if it sent one.
    pub file_name: Option<String>,
    pub bytes: Vec<u8>,
}

impl Fetched {
    /// The document to convert: named after `Content-Disposition` or the
    /// URL path's last segment, its format from that name's extension, else
    /// the `Content-Type`, else the content's signature
    /// ([`InputFormat::sniff`]). The URL is recorded as the base for
    /// relative `<img src>` (resolved only when image fetching is on).
    pub fn into_source(self) -> Result<SourceDocument, crate::ConversionError> {
        let name = self
            .file_name
            .clone()
            .or_else(|| url_file_name(&self.url))
            .unwrap_or_else(|| "document".to_string());
        let path = std::path::Path::new(&name);
        let format = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(InputFormat::from_extension)
            .or_else(|| {
                self.content_type
                    .as_deref()
                    .and_then(InputFormat::from_mime)
            })
            .or_else(|| InputFormat::sniff(&self.bytes))
            .ok_or_else(|| crate::ConversionError::UnknownFormat {
                hint: format!(
                    "cannot tell the format of {}: no known extension, Content-Type {:?}, \
                     and no recognizable signature",
                    self.url,
                    self.content_type.as_deref().unwrap_or("(none)")
                ),
            })?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("document")
            .to_string();
        Ok(SourceDocument::from_bytes(stem, format, self.bytes).with_base_url(self.url))
    }
}

/// Whether a resolved address points back at the local host or private
/// infrastructure: loopback, RFC 1918, link-local (the metadata service),
/// unspecified, broadcast, documentation, carrier-grade NAT, and the IPv6
/// unique-local / link-local ranges — IPv4-mapped IPv6 re-checked as IPv4.
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
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| is_blocked_ip(IpAddr::V4(v4)))
        }
    }
}

/// The pre-connect check: an `http(s)` URL with a host that resolves, to no
/// address [`is_blocked_ip`] refuses (unless
/// `DOCLING_RS_ALLOW_PRIVATE_IP_FETCH` is set). Blocking — it resolves DNS.
pub fn check_url(url: &str) -> Result<(), FetchError> {
    let parsed = url::Url::parse(url).map_err(|e| FetchError(format!("bad URL {url:?}: {e}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(FetchError(format!(
            "unsupported URL scheme {:?} in {url:?} (http and https only)",
            parsed.scheme()
        )));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| FetchError(format!("no host in URL {url:?}")))?;
    let port = parsed.port_or_known_default().unwrap_or(80);
    // `[::1]` arrives bracketed; the resolver wants the bare address.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let addrs: Vec<_> = (bare, port)
        .to_socket_addrs()
        .map_err(|e| FetchError(format!("cannot resolve {host}: {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(FetchError(format!("cannot resolve {host}")));
    }
    if !docling_core::env::flag("DOCLING_RS_ALLOW_PRIVATE_IP_FETCH")
        && addrs.iter().any(|a| is_blocked_ip(a.ip()))
    {
        return Err(FetchError(format!(
            "refusing to fetch {url}: {host} resolves to a private or loopback address \
             (set DOCLING_RS_ALLOW_PRIVATE_IP_FETCH=1 for local development)"
        )));
    }
    Ok(())
}

/// Download `url` under `limits`: [`check_url`] on it and on every redirect
/// target, the body read up to `max_bytes`, connect and overall timeouts.
/// A 4xx/5xx status is an error. Blocking.
pub fn fetch(url: &str, limits: &FetchLimits) -> Result<Fetched, FetchError> {
    fetch_checked(url, limits, &check_url)
}

/// [`fetch`] with the per-hop check passed in — the tests exercise the
/// redirect rule against a local server the real check would refuse.
fn fetch_checked(
    url: &str,
    limits: &FetchLimits,
    check: &dyn Fn(&str) -> Result<(), FetchError>,
) -> Result<Fetched, FetchError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        // Redirects by hand (below), so every hop passes `check_url`.
        .max_redirects(0)
        .timeout_connect(Some(limits.connect_timeout))
        .timeout_global(Some(limits.timeout))
        .build()
        .into();
    let mut target = url.to_string();
    let mut hops = 0;
    let mut response = loop {
        check(&target)?;
        let response = agent
            .get(&target)
            .call()
            .map_err(|e| FetchError(format!("fetching {target}: {e}")))?;
        if !response.status().is_redirection() {
            break response;
        }
        if hops == limits.max_redirects {
            return Err(FetchError(format!(
                "fetching {url}: more than {} redirects",
                limits.max_redirects
            )));
        }
        hops += 1;
        let location = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                FetchError(format!(
                    "fetching {target}: {} without a Location",
                    response.status()
                ))
            })?;
        target = url::Url::parse(&target)
            .and_then(|base| base.join(location))
            .map_err(|e| FetchError(format!("fetching {target}: bad redirect {location:?}: {e}")))?
            .to_string();
    };
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let content_type = header("content-type");
    let file_name = header("content-disposition").and_then(|v| disposition_file_name(&v));
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(limits.max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| FetchError(format!("reading {target}: {e}")))?;
    if bytes.len() as u64 > limits.max_bytes {
        return Err(FetchError(format!(
            "response from {target} exceeds {} bytes (DOCLING_RS_MAX_FETCH_BYTES)",
            limits.max_bytes
        )));
    }
    Ok(Fetched {
        url: target,
        content_type,
        file_name,
        bytes,
    })
}

/// The URL path's last segment, percent-decoded (`/files/Q3%20report.pdf` →
/// `Q3 report.pdf`); `None` for a bare host or a trailing slash.
fn url_file_name(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let segment = parsed.path_segments()?.next_back()?;
    let decoded = percent_decode(segment);
    (!decoded.is_empty()).then_some(decoded)
}

/// The `filename` of a `Content-Disposition` header — the RFC 5987
/// `filename*=UTF-8''…` form first, then the plain (optionally quoted) one;
/// directory parts are dropped so a hostile server cannot name a path.
fn disposition_file_name(value: &str) -> Option<String> {
    let params: Vec<(String, String)> = value
        .split(';')
        .skip(1)
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().to_string()))
        })
        .collect();
    let extended = params
        .iter()
        .find(|(k, _)| k == "filename*")
        .and_then(|(_, v)| {
            let (_, encoded) = v.split_once("''")?;
            Some(percent_decode(encoded))
        });
    let plain = || {
        params
            .iter()
            .find(|(k, _)| k == "filename")
            .map(|(_, v)| v.trim_matches('"').to_string())
    };
    let name = extended.or_else(plain)?;
    let name = name.rsplit(['/', '\\']).next()?.trim().to_string();
    (!name.is_empty() && name != "." && name != "..").then_some(name)
}

/// `%XX` escapes decoded as UTF-8 (lossy).
fn percent_decode(s: &str) -> String {
    String::from_utf8_lossy(&crate::backend::images::percent_decode(s)).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_internal_addresses() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
        ] {
            assert!(is_blocked_ip(ip.parse().unwrap()), "{ip} must be blocked");
        }
        for ip in ["93.184.216.34", "1.1.1.1", "2606:4700::1111"] {
            assert!(!is_blocked_ip(ip.parse().unwrap()), "{ip} is public");
        }
    }

    #[test]
    fn check_url_refuses_before_connecting() {
        // The env escape hatch would lift the address check; these tests
        // only run the scheme/host/address rules with it unset.
        if docling_core::env::flag("DOCLING_RS_ALLOW_PRIVATE_IP_FETCH") {
            return;
        }
        for url in [
            "http://127.0.0.1:9/doc.pdf",
            "http://localhost/doc.pdf",
            "http://[::1]/doc.pdf",
            "http://169.254.169.254/latest/meta-data/",
        ] {
            let err = check_url(url).unwrap_err().to_string();
            assert!(err.contains("private or loopback"), "{url}: {err}");
        }
        let err = check_url("file:///etc/passwd").unwrap_err().to_string();
        assert!(err.contains("unsupported URL scheme"), "{err}");
        assert!(check_url("ftp://example.com/x").is_err());
        assert!(check_url("not a url").is_err());
    }

    #[test]
    fn fetch_refuses_a_private_target() {
        if docling_core::env::flag("DOCLING_RS_ALLOW_PRIVATE_IP_FETCH") {
            return;
        }
        let err = fetch("http://127.0.0.1:9/x.pdf", &FetchLimits::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("private or loopback"), "{err}");
    }

    /// A one-shot HTTP server on loopback: each connection gets the next
    /// canned response.
    fn serve(responses: Vec<&'static str>) -> String {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for (stream, response) in listener.incoming().zip(responses) {
                let mut stream = stream.unwrap();
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    /// The SSRF rule holds on every hop: a permitted URL that redirects to
    /// a refused one fails at the redirect, never fetching it.
    #[test]
    fn every_redirect_hop_is_checked() {
        let base = serve(vec![
            "HTTP/1.1 302 Found\r\nLocation: /internal/secret.pdf\r\nContent-Length: 0\r\n\r\n",
        ]);
        let check = |u: &str| {
            if u.contains("/internal/") {
                Err(FetchError(format!("refused {u}")))
            } else {
                Ok(())
            }
        };
        let err = fetch_checked(&format!("{base}/doc"), &FetchLimits::default(), &check)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("refused") && err.contains("/internal/secret.pdf"),
            "{err}"
        );
    }

    #[test]
    fn redirects_and_size_are_bounded() {
        let allow = |_: &str| Ok(());
        // A redirect loop stops at the cap.
        let hop = "HTTP/1.1 302 Found\r\nLocation: /again\r\nContent-Length: 0\r\n\r\n";
        let base = serve(vec![hop; 3]);
        let limits = FetchLimits {
            max_redirects: 2,
            ..Default::default()
        };
        let err = fetch_checked(&format!("{base}/a"), &limits, &allow)
            .unwrap_err()
            .to_string();
        assert!(err.contains("more than 2 redirects"), "{err}");
        // A body over the cap is refused, not truncated.
        let base = serve(vec![
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 10\r\n\r\n0123456789",
        ]);
        let limits = FetchLimits {
            max_bytes: 4,
            ..Default::default()
        };
        let err = fetch_checked(&format!("{base}/big"), &limits, &allow)
            .unwrap_err()
            .to_string();
        assert!(err.contains("exceeds 4 bytes"), "{err}");
        // The happy path: the final URL, type and name are kept.
        let base = serve(vec![
            "HTTP/1.1 301 Moved\r\nLocation: /files/r.md\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Type: text/markdown\r\nContent-Disposition: attachment; filename=\"notes.md\"\r\nContent-Length: 3\r\n\r\n# x",
        ]);
        let got = fetch_checked(&format!("{base}/old"), &FetchLimits::default(), &allow).unwrap();
        assert_eq!(got.url, format!("{base}/files/r.md"));
        assert_eq!(got.bytes, b"# x");
        assert_eq!(got.file_name.as_deref(), Some("notes.md"));
        assert_eq!(got.content_type.as_deref(), Some("text/markdown"));
        // A 404 is an error, not a document.
        let base = serve(vec!["HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n"]);
        assert!(fetch_checked(&format!("{base}/gone"), &FetchLimits::default(), &allow).is_err());
    }

    #[test]
    fn names_come_from_disposition_then_path() {
        assert_eq!(
            disposition_file_name("attachment; filename=\"Q3 report.pdf\""),
            Some("Q3 report.pdf".into())
        );
        assert_eq!(
            disposition_file_name("attachment; filename=a.pdf; filename*=UTF-8''%C3%BCber.docx"),
            Some("über.docx".into())
        );
        assert_eq!(
            disposition_file_name("attachment; filename=\"../../etc/passwd\""),
            Some("passwd".into())
        );
        assert_eq!(disposition_file_name("inline"), None);
        assert_eq!(
            url_file_name("https://example.com/files/Q3%20report.pdf?x=1#p"),
            Some("Q3 report.pdf".into())
        );
        assert_eq!(url_file_name("https://example.com/"), None);
        assert_eq!(url_file_name("https://example.com"), None);
    }

    #[test]
    fn into_source_detects_the_format() {
        let fetched = |url: &str, ct: Option<&str>, name: Option<&str>, bytes: &[u8]| Fetched {
            url: url.into(),
            content_type: ct.map(str::to_string),
            file_name: name.map(str::to_string),
            bytes: bytes.to_vec(),
        };
        // The extension decides first.
        let s = fetched("https://h/a/notes.md", Some("text/html"), None, b"# x")
            .into_source()
            .unwrap();
        assert_eq!((s.format, s.name.as_str()), (InputFormat::Md, "notes"));
        assert_eq!(s.base_url.as_deref(), Some("https://h/a/notes.md"));
        // No extension: the Content-Type.
        let s = fetched(
            "https://h/view?id=3",
            Some("text/html; charset=utf-8"),
            None,
            b"x",
        )
        .into_source()
        .unwrap();
        assert_eq!(s.format, InputFormat::Html);
        // A Content-Disposition name beats the URL.
        let s = fetched("https://h/dl", None, Some("r.pdf"), b"%PDF-1.4")
            .into_source()
            .unwrap();
        assert_eq!((s.format, s.name.as_str()), (InputFormat::Pdf, "r"));
        // Neither: the signature.
        let s = fetched(
            "https://h/blob",
            Some("application/octet-stream"),
            None,
            b"%PDF-1.4",
        )
        .into_source()
        .unwrap();
        assert_eq!(s.format, InputFormat::Pdf);
        // Nothing to go on: a clear error.
        let err = fetched("https://h/blob", None, None, b"plain words")
            .into_source()
            .unwrap_err()
            .to_string();
        assert!(err.contains("cannot tell the format"), "{err}");
    }
}
