//! ETags for polled JSON (T07).
//!
//! The Activity polls its views every 20 or 60 s. This middleware gives
//! every successful JSON `GET` under `/api` a strong `ETag` (a hash of the
//! body) and answers `304 Not Modified` when the request's `If-None-Match`
//! matches, so an unchanged view costs no body. Responses are marked
//! `private, no-cache`: they depend on the session, and must be revalidated.

use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use crate::error::ApiError;
use crate::hex;

/// Bodies above this size are not buffered. API responses are far smaller.
const MAX_BODY: usize = 16 * 1024 * 1024;

pub async fn layer(req: Request, next: Next) -> Response {
    let is_get = req.method() == Method::GET;
    let if_none_match = req.headers().get(header::IF_NONE_MATCH).cloned();
    let res = next.run(req).await;
    if !is_get || res.status() != StatusCode::OK || !is_json(res.headers()) {
        return res;
    }

    let (mut parts, body) = res.into_parts();
    let bytes = match to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("cannot buffer a response for its ETag: {e}");
            return ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "response too large",
            )
            .into_response();
        }
    };
    let tag = format!("\"{}\"", &hex(&Sha256::digest(&bytes))[..32]);
    let tag = HeaderValue::from_str(&tag).expect("quoted hex is a valid header value");
    parts.headers.insert(header::ETAG, tag.clone());
    parts
        .headers
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("private, no-cache"));

    if if_none_match.is_some_and(|v| matches(&v, &tag)) {
        let mut not_modified = StatusCode::NOT_MODIFIED.into_response();
        for name in [header::ETAG, header::CACHE_CONTROL] {
            if let Some(v) = parts.headers.get(&name) {
                not_modified.headers_mut().insert(name, v.clone());
            }
        }
        return not_modified;
    }
    Response::from_parts(parts, Body::from(bytes))
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/json"))
}

/// Whether an `If-None-Match` value matches `tag`: `*`, or a list of tags,
/// compared weakly (a `W/` prefix is ignored), as RFC 9110 asks for GET.
fn matches(if_none_match: &HeaderValue, tag: &HeaderValue) -> bool {
    let (Ok(list), Ok(tag)) = (if_none_match.to_str(), tag.to_str()) else {
        return false;
    };
    list.split(',').map(str::trim).any(|candidate| {
        candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == tag
    })
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::http::Request;
    use axum::routing::{get, post};
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tower::ServiceExt;

    use super::*;

    fn app(counter: Arc<AtomicU32>) -> Router {
        Router::new()
            .route(
                "/json",
                get(move || {
                    let n = counter.load(Ordering::SeqCst);
                    async move { axum::Json(serde_json::json!({ "n": n })) }
                }),
            )
            .route("/text", get(|| async { "plain" }))
            .route(
                "/post",
                post(|| async { axum::Json(serde_json::json!({})) }),
            )
            .route(
                "/missing",
                get(|| async { (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({}))) }),
            )
            .layer(axum::middleware::from_fn(layer))
    }

    async fn call(
        app: &Router,
        method: &str,
        uri: &str,
        inm: Option<&str>,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(v) = inm {
            req = req.header(header::IF_NONE_MATCH, v);
        }
        let res = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let body = res.into_body().collect().await.unwrap().to_bytes().to_vec();
        (status, headers, body)
    }

    #[tokio::test]
    async fn unchanged_json_is_not_modified() {
        let counter = Arc::new(AtomicU32::new(0));
        let app = app(counter.clone());

        let (status, headers, body) = call(&app, "GET", "/json", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, br#"{"n":0}"#);
        let tag = headers[header::ETAG].to_str().unwrap().to_owned();
        assert!(tag.starts_with('"') && tag.len() == 34, "{tag}");
        assert_eq!(headers[header::CACHE_CONTROL], "private, no-cache");

        // Same body: 304 without a body, with the tag.
        let (status, headers, body) = call(&app, "GET", "/json", Some(&tag)).await;
        assert_eq!(status, StatusCode::NOT_MODIFIED);
        assert!(body.is_empty());
        assert_eq!(headers[header::ETAG], tag.as_str());
        // Lists, weak tags and `*` match too.
        for inm in [format!("\"x\", {tag}"), format!("W/{tag}"), "*".to_owned()] {
            let (status, _, _) = call(&app, "GET", "/json", Some(&inm)).await;
            assert_eq!(status, StatusCode::NOT_MODIFIED, "{inm}");
        }

        // A change: 200 with a new tag.
        counter.store(1, Ordering::SeqCst);
        let (status, headers, body) = call(&app, "GET", "/json", Some(&tag)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, br#"{"n":1}"#);
        assert_ne!(headers[header::ETAG], tag.as_str());
    }

    #[tokio::test]
    async fn other_responses_pass_through() {
        let app = app(Arc::new(AtomicU32::new(0)));
        for (method, uri, status) in [
            ("GET", "/text", StatusCode::OK),
            ("POST", "/post", StatusCode::OK),
            ("GET", "/missing", StatusCode::NOT_FOUND),
        ] {
            let (got, headers, _) = call(&app, method, uri, Some("*")).await;
            assert_eq!(got, status, "{method} {uri}");
            assert!(!headers.contains_key(header::ETAG), "{method} {uri}");
        }
    }
}
