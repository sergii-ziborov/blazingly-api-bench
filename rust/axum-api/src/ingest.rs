//! Scraper ingestion. Auth and the per-key rate limit are a `from_fn_with_state`
//! middleware on the nested `/ingest` router, so neither handler repeats them.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use chrono::DateTime;
use serde::Serialize;
use validator::Validate;

use crate::admin::{build_article, now_rfc3339, reference_errors};
use crate::dto::{BULK_MAX_ITEMS, BULK_MIN_ITEMS, BulkRequest, CreateRun, flatten};
use crate::error::{ApiError, FieldError};
use crate::extract::{ValidJson, api_key};
use crate::state::{AppState, IngestRun};
use crate::view;

pub async fn guard(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let key = api_key(request.headers())?;
    let second = state.started.elapsed().as_secs();
    {
        // Fixed one-second window per key. The happy path never allocates: the
        // key string is only cloned the first time it is seen.
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
    }
    Ok(next.run(request).await)
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

pub async fn bulk(
    State(state): State<AppState>,
    ValidJson(request): ValidJson<BulkRequest>,
) -> Result<Response, ApiError> {
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
        let mut errors = match item.validate() {
            Ok(()) => Vec::new(),
            Err(errors) => flatten(&errors),
        };
        errors.extend(reference_errors(
            &state,
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

    view::ok(&BulkOutcome {
        accepted,
        rejected,
        results,
    })
}

pub async fn create_run(
    State(state): State<AppState>,
    ValidJson(input): ValidJson<CreateRun>,
) -> Result<Response, ApiError> {
    let started = DateTime::parse_from_rfc3339(&input.started_at).map_err(|_| {
        ApiError::field("started_at", "format", "must be an RFC 3339 timestamp")
    })?;
    let finished = DateTime::parse_from_rfc3339(&input.finished_at).map_err(|_| {
        ApiError::field("finished_at", "format", "must be an RFC 3339 timestamp")
    })?;
    if finished < started {
        return Err(ApiError::field(
            "finished_at",
            "before_start",
            "must not precede started_at",
        ));
    }

    let mut runs = state.runs.lock().expect("runs lock");
    let run = IngestRun {
        id: runs.len() as u32 + 1,
        source: input.source,
        started_at: input.started_at,
        finished_at: input.finished_at,
        found: input.found,
        ingested: input.ingested,
        errors: input.errors,
    };
    runs.push(run);
    let stored = runs.last().ok_or(ApiError::Internal)?;
    view::json(StatusCode::CREATED, stored)
}
