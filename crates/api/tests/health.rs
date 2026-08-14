//! Route-level tests: they drive the router directly via `tower`, so no port
//! is bound and the cases stay independent of the ambient environment.

use api::Config;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    routing::get,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;

const REQUEST_ID: &str = "x-request-id";

struct Response {
    status: StatusCode,
    content_type: Option<String>,
    request_id: Option<String>,
    body: Value,
}

async fn send(router: Router, request: Request<Body>) -> Response {
    let response = router.oneshot(request).await.expect("router is infallible");

    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let status = response.status();
    let content_type = header(header::CONTENT_TYPE.as_str());
    let request_id = header(REQUEST_ID);

    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body collects")
        .to_bytes();

    Response {
        status,
        content_type,
        request_id,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

fn get_request(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .body(Body::empty())
        .expect("request builds")
}

async fn get_path(path: &str) -> Response {
    send(api::router(&Config::default()), get_request(path)).await
}

#[tokio::test]
async fn health_reports_ok_with_service_identity() {
    let response = get_path("/health").await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.content_type.as_deref(), Some("application/json"));
    assert_eq!(
        response.body,
        json!({
            "status": "ok",
            "name": "api",
            "version": env!("CARGO_PKG_VERSION"),
        })
    );
}

#[tokio::test]
async fn unknown_path_is_not_found() {
    assert_eq!(get_path("/nope").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn request_id_is_generated_when_absent() {
    let id = get_path("/health")
        .await
        .request_id
        .expect("middleware mints an id when the client sends none");
    assert!(!id.is_empty());
}

#[tokio::test]
async fn client_supplied_request_id_is_preserved() {
    let mut request = get_request("/health");
    request
        .headers_mut()
        .insert(REQUEST_ID, "trace-me-123".parse().expect("valid header"));

    let response = send(api::router(&Config::default()), request).await;
    assert_eq!(response.request_id.as_deref(), Some("trace-me-123"));
}

#[tokio::test]
async fn slow_handler_is_cut_off_by_the_timeout() {
    let config = Config {
        request_timeout: Duration::from_millis(50),
        ..Config::default()
    };
    let slow = Router::new().route(
        "/slow",
        get(|| async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            "unreachable"
        }),
    );

    let response = send(api::apply_middleware(slow, &config), get_request("/slow")).await;

    assert_eq!(response.status, StatusCode::REQUEST_TIMEOUT);
    // Propagation runs innermost so even a synthesized 408 carries the id.
    assert!(response.request_id.is_some());
}
