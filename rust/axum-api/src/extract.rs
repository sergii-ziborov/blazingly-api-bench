//! Custom extractors.
//!
//! `ValidJson` / `ValidQuery` wrap axum's own extractors so that a rejection
//! comes back as `ApiError` instead of axum's plain-text default, and so that
//! `validator` runs before the handler body does. The auth extractors put the
//! role requirement in the handler signature, which is where axum users expect
//! to read it.

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use serde::de::DeserializeOwned;
use validator::Validate;

use crate::dto::validated;
use crate::error::ApiError;
use crate::state::{ADMIN_TOKEN, EDITOR_TOKEN, SCRAPER_KEY};

pub struct ValidJson<T>(pub T);

impl<T, S> FromRequest<S> for ValidJson<T>
where
    T: DeserializeOwned + Validate,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(request, state)
            .await
            .map_err(json_rejection)?;
        Ok(Self(validated(value)?))
    }
}

fn json_rejection(rejection: JsonRejection) -> ApiError {
    match rejection {
        JsonRejection::MissingJsonContentType(_) => {
            ApiError::UnsupportedMediaType("expected content-type: application/json".to_owned())
        }
        // A body that will not parse is a malformed envelope, which the
        // contract makes a 422 rather than axum's default 400.
        other => ApiError::malformed(other.body_text()),
    }
}

pub struct ValidQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ValidQuery<T>
where
    T: DeserializeOwned + Validate,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Query(value) = Query::<T>::from_request_parts(parts, state)
            .await
            .map_err(|rejection: QueryRejection| ApiError::malformed(rejection.body_text()))?;
        Ok(Self(validated(value)?))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Editor,
    Admin,
}

fn editorial_role(parts: &Parts) -> Result<Role, ApiError> {
    let raw = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;
    let token = raw
        .strip_prefix("Bearer ")
        .ok_or(ApiError::Unauthorized)?
        .trim();
    match token {
        EDITOR_TOKEN => Ok(Role::Editor),
        ADMIN_TOKEN => Ok(Role::Admin),
        _ => Err(ApiError::Unauthorized),
    }
}

/// Editor or admin.
pub struct RequireEditor;

impl<S: Send + Sync> FromRequestParts<S> for RequireEditor {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        editorial_role(parts)?;
        Ok(Self)
    }
}

/// Admin only; an authenticated editor gets 403 rather than 401.
pub struct RequireAdmin;

impl<S: Send + Sync> FromRequestParts<S> for RequireAdmin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        match editorial_role(parts)? {
            Role::Admin => Ok(Self),
            Role::Editor => Err(ApiError::Forbidden),
        }
    }
}

/// Ingestion credential, read from a `HeaderMap` so the rate-limit middleware
/// can use it before the request is handed to the handler.
pub fn api_key(headers: &HeaderMap) -> Result<&str, ApiError> {
    let value = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;
    if value == SCRAPER_KEY {
        Ok(value)
    } else {
        Err(ApiError::Unauthorized)
    }
}
