//! Route-level tests: they drive the router directly via `tower`, so no port
//! is bound and the cases stay independent of the ambient environment.

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn get(path: &str) -> (StatusCode, Option<String>, Value) {
    let response = api::router()
        .oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router is infallible");

    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body collects")
        .to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);

    (status, content_type, body)
}

#[tokio::test]
async fn health_reports_ok_with_service_identity() {
    let (status, content_type, body) = get("/health").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("application/json"));
    assert_eq!(
        body,
        json!({
            "status": "ok",
            "name": "api",
            "version": env!("CARGO_PKG_VERSION"),
        })
    );
}

#[tokio::test]
async fn unknown_path_is_not_found() {
    let (status, _, _) = get("/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
