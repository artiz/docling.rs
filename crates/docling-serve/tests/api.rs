//! Router-level tests over `tower::ServiceExt::oneshot` — no sockets, no ML
//! models: the conversions exercised here are declarative (Markdown/HTML/CSV
//! uploads), so the suite runs in plain CI.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use docling_serve::{router, ServeConfig};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn app() -> axum::Router {
    router(ServeConfig::default())
}

async fn body_string(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// A multipart body with one `file` part and optional extra text parts.
fn multipart(file_name: &str, content: &[u8], fields: &[(&str, &str)]) -> (String, Vec<u8>) {
    let boundary = "docling-serve-test-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(content);
    for (k, v) in fields {
        body.extend_from_slice(
            format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}")
                .as_bytes(),
        );
    }
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn convert_request(content_type: &str, body: Vec<u8>, query: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/v1/convert{query}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn health_is_ok() {
    let response = app()
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response).await.contains("ok"));
}

#[tokio::test]
async fn ready_without_warmup_is_immediate() {
    let response = app()
        .oneshot(Request::get("/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn converts_markdown_upload_to_markdown() {
    let (ct, body) = multipart("note.md", b"# Title\n\nHello *world*.\n", &[]);
    let response = app().oneshot(convert_request(&ct, body, "")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/markdown; charset=utf-8"
    );
    let out = body_string(response).await;
    assert!(out.contains("# Title"), "unexpected body: {out}");
}

#[tokio::test]
async fn converts_csv_to_docling_json() {
    let (ct, body) = multipart("t.csv", b"a,b\n1,2\n", &[("to", "json")]);
    let response = app().oneshot(convert_request(&ct, body, "")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body_string(response).await).unwrap();
    assert_eq!(v["schema_name"], "DoclingDocument");
}

#[tokio::test]
async fn query_options_apply_and_body_wins() {
    // Query says json, body field says chunks — body wins.
    let (ct, body) = multipart("t.csv", b"a,b\n1,2\n", &[("to", "chunks")]);
    let response = app()
        .oneshot(convert_request(&ct, body, "?to=json"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body_string(response).await).unwrap();
    assert!(v.get("hierarchical").is_some(), "chunks shape expected");
}

#[tokio::test]
async fn dclx_download_has_attachment_headers() {
    let (ct, body) = multipart("sheet.csv", b"a,b\n1,2\n", &[("to", "dclx")]);
    let response = app().oneshot(convert_request(&ct, body, "")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"sheet.dclx\""
    );
    // A dclx archive is a ZIP: PK magic.
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..2], b"PK");
}

#[tokio::test]
async fn unknown_format_is_422() {
    let (ct, body) = multipart("data.xyz", b"?", &[]);
    let response = app().oneshot(convert_request(&ct, body, "")).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn missing_file_part_is_400() {
    let (ct, body) = multipart("x.md", b"x", &[]);
    // Rewrite the part name so no `file` part arrives.
    let body = String::from_utf8(body)
        .unwrap()
        .replace("name=\"file\"", "name=\"data\"");
    let response = app()
        .oneshot(convert_request(&ct, body.into_bytes(), ""))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn bad_to_value_is_400() {
    let (ct, body) = multipart("x.md", b"x", &[("to", "pdf")]);
    let response = app().oneshot(convert_request(&ct, body, "")).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn url_fetch_can_be_disabled() {
    let cfg = ServeConfig {
        allow_url_fetch: false,
        ..ServeConfig::default()
    };
    let response = router(cfg)
        .oneshot(convert_request(
            "application/json",
            br#"{"url": "https://example.com/x.md"}"#.to_vec(),
            "",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn wrong_content_type_is_400() {
    let response = app()
        .oneshot(convert_request("text/plain", b"hello".to_vec(), ""))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn strict_field_changes_markdown_dialect() {
    // Legacy docling output escapes the underscore in `x_y`; strict mode
    // doesn't. The exact difference doesn't matter here, only that the switch
    // reaches the converter.
    let md = b"x_y and 5*6\n";
    let (ct1, b1) = multipart("p.md", md, &[]);
    let (ct2, b2) = multipart("p.md", md, &[("strict", "true")]);
    let legacy = body_string(app().oneshot(convert_request(&ct1, b1, "")).await.unwrap()).await;
    let strict = body_string(app().oneshot(convert_request(&ct2, b2, "")).await.unwrap()).await;
    assert_ne!(legacy, strict, "strict flag had no effect");
}

/// `fetch_images` is outbound fetch (SSRF surface), so it's gated behind the
/// same `--allow-url-fetch` as URL inputs: honored only when the flag is on,
/// silently ignored otherwise. Proven against a local image server that counts
/// the requests it receives — the gate must let *zero* through when off.
#[tokio::test]
async fn fetch_images_is_gated_behind_allow_url_fetch() {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // 1×1 red PNG — a real image so the resolved bytes decode and embed.
    const RED_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x00, 0x00, 0x03, 0x00, 0x01, 0x6e, 0x2c, 0xdc, 0x33, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let server_hits = Arc::clone(&hits);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            server_hits.fetch_add(1, Ordering::Relaxed);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                RED_PNG.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(RED_PNG);
            let _ = stream.flush();
        }
    });
    let html = format!("<html><body><p>hi</p><img src=\"http://{addr}/x.png\"></body></html>");
    let fields = [("to", "json"), ("fetch_images", "true")];

    // Gate OFF (secure default): fetch_images asked for, but --allow-url-fetch
    // is off → no outbound fetch, the picture stays a placeholder (no bytes).
    let (ct, body) = multipart("p.html", html.as_bytes(), &fields);
    let cfg = ServeConfig {
        allow_url_fetch: false,
        ..ServeConfig::default()
    };
    let response = router(cfg)
        .oneshot(convert_request(&ct, body, ""))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let out = body_string(response).await;
    assert!(
        !out.contains("data:image/"),
        "gate off must not embed: {out}"
    );
    assert_eq!(
        hits.load(Ordering::Relaxed),
        0,
        "gate off must not fetch the image"
    );

    // Gate ON: --allow-url-fetch set (plus the private-IP opt-in for 127.0.0.1)
    // → the image is fetched and embedded as a data URI.
    std::env::set_var("DOCLING_RS_ALLOW_PRIVATE_IP_FETCH", "1");
    let (ct, body) = multipart("p.html", html.as_bytes(), &fields);
    let cfg = ServeConfig {
        allow_url_fetch: true,
        ..ServeConfig::default()
    };
    let response = router(cfg)
        .oneshot(convert_request(&ct, body, ""))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let out = body_string(response).await;
    std::env::remove_var("DOCLING_RS_ALLOW_PRIVATE_IP_FETCH");
    assert!(
        hits.load(Ordering::Relaxed) >= 1,
        "gate on must fetch the image"
    );
    assert!(
        out.contains("data:image/"),
        "gate on must embed the fetched image"
    );
}

#[tokio::test]
async fn index_serves_docs_and_form() {
    let response = app()
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("/v1/convert") && body.contains("<form") || body.contains("Convert"));
}
