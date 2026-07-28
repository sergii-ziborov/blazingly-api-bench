//! Method-and-path dispatch, written by hand.
//!
//! This is the part a framework is usually bought for. There is no route table
//! and no matcher: the path is split into at most four segments once, and the
//! arms below match on the segment count and the literal segments. Path
//! parameters come out as `&str` and are converted where they are used.
//!
//! The ordering constraints the frameworks encode in extractor position are
//! explicit here instead — auth is checked before the body is read, so an
//! unauthenticated request is a 401 without the service ever touching its
//! payload.

use bytes::Bytes;
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper::http::request::Parts;
use hyper::{HeaderMap, Method, Request, Response, header};
use serde::de::DeserializeOwned;

use crate::body::ResBody;
use crate::error::ApiError;
use crate::query::decode_path;
use crate::store::{ADMIN_TOKEN, AppState, EDITOR_TOKEN};
use crate::validate::MAX_JSON_BYTES;
use crate::{admin, ingest, ops, public};

/// `/admin/articles/{id}/cover` is the longest route in the contract.
const MAX_SEGMENTS: usize = 4;

pub async fn handle(
    state: AppState,
    request: Request<Incoming>,
) -> Result<Response<ResBody>, std::convert::Infallible> {
    Ok(match dispatch(&state, request).await {
        Ok(response) => response,
        Err(error) => error.render(),
    })
}

async fn dispatch(
    state: &AppState,
    request: Request<Incoming>,
) -> Result<Response<ResBody>, ApiError> {
    let (parts, incoming) = request.into_parts();

    let mut segments = [""; MAX_SEGMENTS];
    let mut count = 0_usize;
    for segment in parts.uri.path().split('/').skip(1) {
        if count == MAX_SEGMENTS {
            return Err(ApiError::NotFound("route"));
        }
        segments[count] = segment;
        count += 1;
    }
    let raw_query = parts.uri.query().unwrap_or("");

    match (count, segments[0]) {
        (1, "health") => {
            expect(&parts, Method::GET)?;
            Ok(ops::health(state))
        }
        (1, "events") => {
            expect(&parts, Method::GET)?;
            Ok(ops::events(state))
        }
        (1, "articles") => {
            expect(&parts, Method::GET)?;
            public::list_articles(state, raw_query)
        }
        (2, "articles") => {
            expect(&parts, Method::GET)?;
            public::article_detail(state, &decode_path(segments[1]))
        }
        (1, "categories") => {
            expect(&parts, Method::GET)?;
            Ok(public::categories(state))
        }
        (1, "tags") => {
            expect(&parts, Method::GET)?;
            Ok(public::tags(state))
        }
        (2, "authors") => {
            expect(&parts, Method::GET)?;
            public::author(state, &decode_path(segments[1]))
        }
        (1, "companies") => {
            expect(&parts, Method::GET)?;
            public::list_companies(state, raw_query)
        }
        (1, "search") => {
            expect(&parts, Method::GET)?;
            public::search(state, raw_query)
        }

        (2, "admin") if segments[1] == "articles" => {
            expect(&parts, Method::POST)?;
            require_editor(&parts.headers)?;
            let payload = read_json(&parts.headers, incoming).await?;
            admin::create_article(state, &payload)
        }
        (3, "admin") if segments[1] == "articles" => {
            let id = article_id(segments[2])?;
            match parts.method {
                Method::PATCH => {
                    require_editor(&parts.headers)?;
                    let payload = read_json(&parts.headers, incoming).await?;
                    admin::update_article(state, id, &payload)
                }
                Method::DELETE => {
                    require_admin(&parts.headers)?;
                    admin::delete_article(state, id)
                }
                _ => Err(ApiError::MethodNotAllowed),
            }
        }
        (4, "admin") if segments[1] == "articles" && segments[3] == "cover" => {
            expect(&parts, Method::POST)?;
            let id = article_id(segments[2])?;
            require_editor(&parts.headers)?;
            let content_type = header_str(&parts.headers, &header::CONTENT_TYPE);
            admin::upload_cover(state, id, content_type, incoming).await
        }
        (4, "admin") if segments[1] == "articles" && segments[3] == "publish" => {
            expect(&parts, Method::POST)?;
            let id = article_id(segments[2])?;
            require_editor(&parts.headers)?;
            let payload = read_json(&parts.headers, incoming).await?;
            admin::publish_article(state, id, &payload)
        }

        (3, "ingest") if segments[1] == "articles" && segments[2] == "bulk" => {
            expect(&parts, Method::POST)?;
            ingest::guard(state, &parts.headers)?;
            let payload = read_json(&parts.headers, incoming).await?;
            ingest::bulk(state, &payload)
        }
        (2, "ingest") if segments[1] == "runs" => {
            expect(&parts, Method::POST)?;
            ingest::guard(state, &parts.headers)?;
            let payload = read_json(&parts.headers, incoming).await?;
            ingest::create_run(state, &payload)
        }

        _ => Err(ApiError::NotFound("route")),
    }
}

fn expect(parts: &Parts, method: Method) -> Result<(), ApiError> {
    if parts.method == method {
        Ok(())
    } else {
        Err(ApiError::MethodNotAllowed)
    }
}

fn article_id(segment: &str) -> Result<u32, ApiError> {
    segment
        .parse::<u32>()
        .map_err(|_| ApiError::NotFound("article"))
}

fn header_str<'a>(headers: &'a HeaderMap, name: &header::HeaderName) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

// --- credentials -----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Editor,
    Admin,
}

fn editorial_role(headers: &HeaderMap) -> Result<Role, ApiError> {
    let raw = header_str(headers, &header::AUTHORIZATION).ok_or(ApiError::Unauthorized)?;
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
fn require_editor(headers: &HeaderMap) -> Result<(), ApiError> {
    editorial_role(headers).map(|_| ())
}

/// Admin only; an authenticated editor gets 403 rather than 401.
fn require_admin(headers: &HeaderMap) -> Result<(), ApiError> {
    match editorial_role(headers)? {
        Role::Admin => Ok(()),
        Role::Editor => Err(ApiError::Forbidden),
    }
}

// --- request bodies --------------------------------------------------------

fn is_json(content_type: &str) -> bool {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    essence == "application/json"
        || (essence.starts_with("application/") && essence.ends_with("+json"))
}

async fn read_json(headers: &HeaderMap, incoming: Incoming) -> Result<Bytes, ApiError> {
    let content_type = header_str(headers, &header::CONTENT_TYPE).unwrap_or_default();
    if !is_json(content_type) {
        return Err(ApiError::UnsupportedMediaType(
            "expected content-type: application/json".to_owned(),
        ));
    }
    match Limited::new(incoming, MAX_JSON_BYTES).collect().await {
        Ok(collected) => Ok(collected.to_bytes()),
        Err(error) if error.is::<http_body_util::LengthLimitError>() => Err(
            ApiError::PayloadTooLarge(format!("body must not exceed {MAX_JSON_BYTES} bytes")),
        ),
        Err(error) => Err(ApiError::BadRequest(format!(
            "failed to read the request body: {error}"
        ))),
    }
}

pub fn from_json<T: DeserializeOwned>(payload: &Bytes) -> Result<T, ApiError> {
    serde_json::from_slice(payload).map_err(|error| {
        ApiError::malformed(format!(
            "failed to deserialize the JSON body into the target type: {error}"
        ))
    })
}
