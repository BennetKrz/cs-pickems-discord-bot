//! The JSON error body shared by every API route.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// A JSON error response: `{"error": code, "message": text}`.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    /// A 500 for a failed database call. The details go to the log only.
    pub fn db(context: &str, e: sqlx::Error) -> Self {
        tracing::error!("{context}: database error: {e}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "database error",
        )
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

/// Like [`axum::Json`] as an extractor, but the body must be a JSON object,
/// and a body that is not valid for `T` answers with the JSON error body
/// (`invalid_body`) instead of axum's plain-text rejection. (Serde would
/// otherwise also accept a struct written as an array.)
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonBody<T>(pub T);

impl<S, T> axum::extract::FromRequest<S> for JsonBody<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, ApiError> {
        let invalid = |status, message: String| ApiError::new(status, "invalid_body", message);
        let value = match Json::<serde_json::Value>::from_request(req, state).await {
            Ok(Json(v)) => v,
            Err(e) => return Err(invalid(e.status(), e.body_text())),
        };
        if !value.is_object() {
            return Err(invalid(
                StatusCode::UNPROCESSABLE_ENTITY,
                "the body must be a JSON object".to_owned(),
            ));
        }
        serde_json::from_value(value)
            .map(Self)
            .map_err(|e| invalid(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.code, "message": self.message });
        (self.status, Json(body)).into_response()
    }
}
