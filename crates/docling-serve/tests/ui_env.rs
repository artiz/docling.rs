//! #892: the web UI switched off from the environment — upstream
//! docling-serve's `DOCLING_SERVE_ENABLE_UI=0` (a docker-compose setting)
//! and ours, `DOCLING_RS_ENABLE_UI`, which wins over it. Its own test binary:
//! the variables are process-wide, and the router tests in `api.rs` run in
//! parallel threads of one process.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use docling_serve::{router, ServeConfig};
use tower::ServiceExt;

async fn root_status(cfg: ServeConfig) -> StatusCode {
    router(cfg)
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn enable_ui_variables_and_their_precedence() {
    std::env::set_var("DOCLING_SERVE_ENABLE_UI", "0");
    assert_eq!(
        root_status(ServeConfig::default()).await,
        StatusCode::NOT_FOUND,
        "upstream's variable turns the page off"
    );
    // The flag (`--enable-ui`) beats the environment.
    let forced = ServeConfig {
        ui: Some(true),
        ..ServeConfig::default()
    };
    assert_eq!(root_status(forced).await, StatusCode::OK);
    // Ours wins over upstream's.
    std::env::set_var("DOCLING_RS_ENABLE_UI", "1");
    assert_eq!(root_status(ServeConfig::default()).await, StatusCode::OK);
    std::env::set_var("DOCLING_RS_ENABLE_UI", "false");
    std::env::set_var("DOCLING_SERVE_ENABLE_UI", "1");
    assert_eq!(
        root_status(ServeConfig::default()).await,
        StatusCode::NOT_FOUND
    );
    std::env::remove_var("DOCLING_RS_ENABLE_UI");
    std::env::remove_var("DOCLING_SERVE_ENABLE_UI");
    assert_eq!(root_status(ServeConfig::default()).await, StatusCode::OK);
}
