//! Editorial auth as extractors.
//!
//! `Editor` / `Admin` are zero-sized guards: putting one in a handler signature
//! is the whole authorisation statement, and the 401/403 comes back through
//! `ApiError`'s `ResponseError` impl without the handler doing anything.

use std::future::{Ready, ready};

use actix_web::{FromRequest, HttpRequest, dev::Payload, http::header};

use crate::error::ApiError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Editor,
    Admin,
}

fn role_of(req: &HttpRequest) -> Result<Role, ApiError> {
    let raw = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::Unauthorized("missing bearer token"))?;

    match raw.strip_prefix("Bearer ").map(str::trim) {
        Some("editor-token") => Ok(Role::Editor),
        Some("admin-token") => Ok(Role::Admin),
        _ => Err(ApiError::Unauthorized("invalid bearer token")),
    }
}

/// Requires role `editor` or better.
pub struct Editor(#[allow(dead_code)] pub Role);

impl FromRequest for Editor {
    type Error = ApiError;
    type Future = Ready<Result<Self, ApiError>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        ready(role_of(req).map(Editor))
    }
}

/// Requires role `admin`; an authenticated editor gets 403, not 401.
pub struct Admin;

impl FromRequest for Admin {
    type Error = ApiError;
    type Future = Ready<Result<Self, ApiError>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        ready(match role_of(req) {
            Ok(Role::Admin) => Ok(Admin),
            Ok(Role::Editor) => Err(ApiError::Forbidden("admin role required")),
            Err(err) => Err(err),
        })
    }
}

pub const API_KEY_HEADER: &str = "x-api-key";
pub const API_KEY: &str = "scraper-key";
