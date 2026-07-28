//! One error enum for the whole service, implementing `IntoResponse`.
//!
//! Every handler returns `Result<_, ApiError>`, so `?` on a lookup that missed
//! is enough to produce the contract's 404.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// The per-field shape the bulk endpoint is required to emit. Reused for every
/// other 422 so the error body is uniform across the API.
#[derive(Debug, Clone, Serialize)]
pub struct FieldError {
    pub field: String,
    pub code: String,
    pub message: String,
}

impl FieldError {
    pub fn new(field: impl Into<String>, code: &str, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

#[derive(Debug)]
pub enum ApiError {
    NotFound(&'static str),
    Unauthorized,
    Forbidden,
    Conflict(&'static str),
    Unprocessable {
        message: String,
        errors: Vec<FieldError>,
    },
    BadRequest(String),
    UnsupportedMediaType(String),
    PayloadTooLarge(String),
    TooManyRequests,
    Internal,
}

impl ApiError {
    pub fn validation(errors: Vec<FieldError>) -> Self {
        Self::Unprocessable {
            message: "request failed validation".to_owned(),
            errors,
        }
    }

    pub fn field(field: &str, code: &str, message: &str) -> Self {
        Self::Unprocessable {
            message: "request failed validation".to_owned(),
            errors: vec![FieldError::new(field, code, message)],
        }
    }

    pub fn malformed(message: impl Into<String>) -> Self {
        Self::Unprocessable {
            message: message.into(),
            errors: Vec::new(),
        }
    }

    fn parts(self) -> (StatusCode, &'static str, String, Vec<FieldError>) {
        match self {
            Self::NotFound(what) => (
                StatusCode::NOT_FOUND,
                "not_found",
                format!("{what} not found"),
                Vec::new(),
            ),
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing or invalid credentials".to_owned(),
                Vec::new(),
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "role is not permitted to perform this action".to_owned(),
                Vec::new(),
            ),
            Self::Conflict(message) => (
                StatusCode::CONFLICT,
                "conflict",
                message.to_owned(),
                Vec::new(),
            ),
            Self::Unprocessable { message, errors } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_failed",
                message,
                errors,
            ),
            Self::BadRequest(message) => {
                (StatusCode::BAD_REQUEST, "bad_request", message, Vec::new())
            }
            Self::UnsupportedMediaType(message) => (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
                message,
                Vec::new(),
            ),
            Self::PayloadTooLarge(message) => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                message,
                Vec::new(),
            ),
            Self::TooManyRequests => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "rate limit exceeded".to_owned(),
                Vec::new(),
            ),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal server error".to_owned(),
                Vec::new(),
            ),
        }
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    details: Vec<FieldError>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message, details) = self.parts();
        let body = serde_json::to_vec(&ErrorBody {
            error: code,
            message,
            details,
        })
        .unwrap_or_else(|_| br#"{"error":"internal"}"#.to_vec());

        let mut response = (
            status,
            [(header::CONTENT_TYPE, "application/json")],
            body,
        )
            .into_response();
        if status == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        response
    }
}
