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

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.code, "message": self.message });
        (self.status, Json(body)).into_response()
    }
}
