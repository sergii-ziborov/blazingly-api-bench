//! One error enum for the whole service, surfaced through Actix's
//! [`ResponseError`] trait so every handler can just return `Result<_, ApiError>`
//! and `?` its way out.

use std::fmt;

use actix_web::{HttpResponse, ResponseError, http::StatusCode};
use serde::Serialize;

/// Per-field validation failure. Shared by the 422 body and by the per-item
/// `results[].errors` array of the bulk ingestion endpoint, which the harness
/// asserts on.
#[derive(Debug, Clone, Serialize)]
pub struct FieldError {
    pub field: String,
    pub code: &'static str,
    pub message: String,
}

impl FieldError {
    pub fn new(field: impl Into<String>, code: &'static str, message: impl Into<String>) -> Self {
        Self { field: field.into(), code, message: message.into() }
    }
}

#[derive(Debug)]
pub enum ApiError {
    /// Missing or unrecognised credentials.
    Unauthorized(&'static str),
    /// Authenticated but the role is not sufficient.
    Forbidden(&'static str),
    NotFound(&'static str),
    Conflict(&'static str),
    /// Malformed request that never reached field validation (bad JSON, bad
    /// multipart framing).
    BadRequest(String),
    UnsupportedMediaType(String),
    PayloadTooLarge(usize),
    TooManyRequests(u64),
    Unprocessable { message: String, fields: Vec<FieldError> },
}

impl ApiError {
    pub fn invalid(fields: Vec<FieldError>) -> Self {
        ApiError::Unprocessable { message: "validation failed".into(), fields }
    }

    pub fn invalid_field(field: &str, code: &'static str, message: impl Into<String>) -> Self {
        ApiError::invalid(vec![FieldError::new(field, code, message)])
    }

    fn code(&self) -> &'static str {
        match self {
            ApiError::Unauthorized(_) => "unauthorized",
            ApiError::Forbidden(_) => "forbidden",
            ApiError::NotFound(_) => "not_found",
            ApiError::Conflict(_) => "conflict",
            ApiError::BadRequest(_) => "bad_request",
            ApiError::UnsupportedMediaType(_) => "unsupported_media_type",
            ApiError::PayloadTooLarge(_) => "payload_too_large",
            ApiError::TooManyRequests(_) => "rate_limited",
            ApiError::Unprocessable { .. } => "validation_failed",
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Unauthorized(what) => write!(f, "{what}"),
            ApiError::Forbidden(what) => write!(f, "{what}"),
            ApiError::NotFound(what) => write!(f, "{what} not found"),
            ApiError::Conflict(what) => write!(f, "{what}"),
            ApiError::BadRequest(msg) => write!(f, "{msg}"),
            ApiError::UnsupportedMediaType(ct) => {
                write!(f, "unsupported content type: {ct}")
            }
            ApiError::PayloadTooLarge(max) => write!(f, "payload exceeds {max} bytes"),
            ApiError::TooManyRequests(after) => {
                write!(f, "rate limit exceeded, retry after {after}s")
            }
            ApiError::Unprocessable { message, .. } => write!(f, "{message}"),
        }
    }
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: ErrorDetail<'a>,
}

#[derive(Serialize)]
struct ErrorDetail<'a> {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "no_fields")]
    fields: &'a [FieldError],
}

fn no_fields(fields: &&[FieldError]) -> bool {
    fields.is_empty()
}

impl ResponseError for ApiError {
    fn status_code(&self) -> StatusCode {
        match self {
            ApiError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::UnsupportedMediaType(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ApiError::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            ApiError::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            ApiError::Unprocessable { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        }
    }

    fn error_response(&self) -> HttpResponse {
        const NONE: &[FieldError] = &[];
        let fields = match self {
            ApiError::Unprocessable { fields, .. } => fields.as_slice(),
            _ => NONE,
        };
        let mut builder = HttpResponse::build(self.status_code());
        if let ApiError::Unauthorized(_) = self {
            builder.insert_header(("www-authenticate", "Bearer"));
        }
        if let ApiError::TooManyRequests(retry_after) = self {
            builder.insert_header(("retry-after", retry_after.to_string()));
        }
        builder.json(ErrorBody {
            error: ErrorDetail { code: self.code(), message: self.to_string(), fields },
        })
    }
}
