//! Scraper ingestion.
//!
//! The framework implementations put auth and the per-key rate limit in a
//! middleware layer attached to a nested router. There is no layer to attach
//! to here, so `guard` is called explicitly from the two `/ingest` router arms.

use bytes::Bytes;
use chrono::DateTime;
use hyper::{HeaderMap, Response, StatusCode};
use serde::Serialize;

use crate::admin::{build_article, now_rfc3339};
use crate::body::{self, ResBody};
use crate::error::{ApiError, FieldError};
use crate::store::{AppState, IngestRun, SCRAPER_KEY};
use crate::validate::{
    self, BULK_MAX_ITEMS, BULK_MIN_ITEMS, BulkRequest, CreateRun, reference_errors,
};

/// Ingestion credential plus the fixed one-second window per key. The happy
/// path never allocates: the key string is only cloned the first time it is
/// seen.
pub fn guard(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let key = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;
    if key != SCRAPER_KEY {
        return Err(ApiError::Unauthorized);
    }

    let second = state.started.elapsed().as_secs();
    let mut limiter = state.limiter.lock().expect("limiter lock");
    if let Some(counter) = limiter.get_mut(key) {
        if counter.0 != second {
            *counter = (second, 0);
        }
        counter.1 += 1;
        if counter.1 > state.rate_limit {
            return Err(ApiError::TooManyRequests);
        }
    } else {
        limiter.insert(key.to_owned(), (second, 1));
    }
    Ok(())
}

#[derive(Serialize)]
struct ItemOutcome<'a> {
    index: usize,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slug: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<FieldError>,
}

#[derive(Serialize)]
struct BulkOutcome<'a> {
    accepted: usize,
    rejected: usize,
    results: Vec<ItemOutcome<'a>>,
}

pub fn bulk(state: &AppState, payload: &Bytes) -> Result<Response<ResBody>, ApiError> {
    let request: BulkRequest = crate::router::from_json(payload)?;
    if request.items.len() < BULK_MIN_ITEMS || request.items.len() > BULK_MAX_ITEMS {
        return Err(ApiError::field(
            "items",
            "length",
            "must contain between 1 and 100 items",
        ));
    }

    let now = now_rfc3339();
    let mut articles = state.articles.write().expect("articles lock");
    let mut results = Vec::with_capacity(request.items.len());
    let mut accepted = 0_usize;
    let mut rejected = 0_usize;

    for (index, item) in request.items.iter().enumerate() {
        let mut errors = validate::shape_errors(item);
        errors.extend(reference_errors(
            state,
            &articles,
            Some(item.category_id),
            Some(item.author_id),
            Some(&item.tag_ids),
            None,
        ));

        if !errors.is_empty() {
            rejected += 1;
            results.push(ItemOutcome {
                index,
                status: "rejected",
                id: None,
                slug: None,
                errors,
            });
            continue;
        }

        // A slug that already exists is reported as its own outcome rather than
        // as a validation failure, per the contract.
        if articles.contains_slug(&item.slug) {
            rejected += 1;
            results.push(ItemOutcome {
                index,
                status: "duplicate",
                id: None,
                slug: Some(item.slug.as_str()),
                errors: Vec::new(),
            });
            continue;
        }

        let id = articles.next_id();
        articles.insert(build_article(id, item, &now));
        accepted += 1;
        results.push(ItemOutcome {
            index,
            status: "created",
            id: Some(id),
            slug: Some(item.slug.as_str()),
            errors: Vec::new(),
        });
    }

    Ok(body::ok(&BulkOutcome {
        accepted,
        rejected,
        results,
    }))
}

pub fn create_run(state: &AppState, payload: &Bytes) -> Result<Response<ResBody>, ApiError> {
    let input: CreateRun = crate::router::from_json(payload)?;
    validate::fail_if(validate::run_shape_errors(&input))?;

    let started = DateTime::parse_from_rfc3339(&input.started_at)
        .map_err(|_| ApiError::field("started_at", "format", "must be an RFC 3339 timestamp"))?;
    let finished = DateTime::parse_from_rfc3339(&input.finished_at)
        .map_err(|_| ApiError::field("finished_at", "format", "must be an RFC 3339 timestamp"))?;
    if finished < started {
        return Err(ApiError::field(
            "finished_at",
            "before_start",
            "must not precede started_at",
        ));
    }

    let mut runs = state.runs.lock().expect("runs lock");
    let id = runs.len() as u32 + 1;
    runs.push(IngestRun {
        id,
        source: input.source,
        started_at: input.started_at,
        finished_at: input.finished_at,
        found: input.found,
        ingested: input.ingested,
        errors: input.errors,
    });
    let stored = runs.last().ok_or(ApiError::Internal)?;
    Ok(body::json(StatusCode::CREATED, stored))
}
