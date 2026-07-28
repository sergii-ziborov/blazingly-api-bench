//! Scraper ingestion surface, mounted under `web::scope("/ingest")`.
//!
//! Both the API key and the rate limit are scope-wide policy rather than
//! per-handler concerns, so they live in one `middleware::from_fn` wrapped
//! around the scope.

use actix_web::body::MessageBody;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::middleware::Next;
use actix_web::{Error, HttpResponse, post, web};
use serde::Serialize;

use crate::auth::{API_KEY, API_KEY_HEADER};
use crate::clock;
use crate::error::{ApiError, FieldError};
use crate::routes::admin::new_article;
use crate::store::{AppState, IngestRun};
use crate::validation::{self, BulkRequest, CreateArticle, RunRequest};

pub async fn guard<B>(req: ServiceRequest, next: Next<B>) -> Result<ServiceResponse<B>, Error>
where
    B: MessageBody,
{
    let key = req
        .headers()
        .get(API_KEY_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::Unauthorized("missing X-API-Key"))?;
    if key != API_KEY {
        return Err(ApiError::Unauthorized("invalid X-API-Key").into());
    }

    let state = req.app_data::<web::Data<AppState>>().expect("app state is registered");
    if !state.limiter.check(key) {
        return Err(ApiError::TooManyRequests(1).into());
    }

    next.call(req).await
}

#[derive(Serialize)]
struct ItemResult {
    index: usize,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slug: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<FieldError>,
}

impl ItemResult {
    fn created(index: usize, id: u64, slug: String) -> Self {
        Self { index, status: "created", id: Some(id), slug: Some(slug), errors: Vec::new() }
    }

    fn duplicate(index: usize, slug: String) -> Self {
        Self { index, status: "duplicate", id: None, slug: Some(slug), errors: Vec::new() }
    }

    fn rejected(index: usize, errors: Vec<FieldError>) -> Self {
        Self { index, status: "rejected", id: None, slug: None, errors }
    }
}

#[derive(Serialize)]
struct BulkResponse {
    accepted: usize,
    rejected: usize,
    results: Vec<ItemResult>,
}

#[post("/articles/bulk")]
async fn bulk(
    state: web::Data<AppState>,
    body: web::Json<BulkRequest>,
) -> Result<HttpResponse, ApiError> {
    let items = body.into_inner().items;
    if items.is_empty() || items.len() > 100 {
        return Err(ApiError::invalid_field("items", "range", "must hold 1 to 100 entries"));
    }

    let mut store = state.write();
    let mut results = Vec::with_capacity(items.len());
    let mut accepted = 0usize;

    for (index, raw) in items.iter().enumerate() {
        // Each item is parsed on its own so a single malformed entry is one
        // rejected result rather than a failed batch.
        let input: CreateArticle = match serde_json::from_str(raw.get()) {
            Ok(input) => input,
            Err(err) => {
                results.push(ItemResult::rejected(
                    index,
                    vec![FieldError::new("item", "malformed", err.to_string())],
                ));
                continue;
            }
        };

        let errors = validation::validate_create(&store, &input);
        if !errors.is_empty() {
            results.push(ItemResult::rejected(index, errors));
            continue;
        }
        if store.article_by_slug(&input.slug).is_some() {
            results.push(ItemResult::duplicate(index, input.slug));
            continue;
        }

        let id = store.next_article_id();
        let slug = input.slug.clone();
        store.insert_article(new_article(id, input));
        accepted += 1;
        results.push(ItemResult::created(index, id, slug));
    }

    Ok(HttpResponse::Ok().json(BulkResponse {
        accepted,
        rejected: results.len() - accepted,
        results,
    }))
}

#[post("/runs")]
async fn create_run(
    state: web::Data<AppState>,
    body: web::Json<RunRequest>,
) -> Result<HttpResponse, ApiError> {
    let input = body.into_inner();
    let started = clock::parse(&input.started_at).ok_or_else(|| {
        ApiError::invalid_field("started_at", "format", "must be an RFC 3339 timestamp")
    })?;
    let finished = clock::parse(&input.finished_at).ok_or_else(|| {
        ApiError::invalid_field("finished_at", "format", "must be an RFC 3339 timestamp")
    })?;
    if finished < started {
        return Err(ApiError::invalid_field(
            "finished_at",
            "order",
            "must not precede started_at",
        ));
    }

    let mut store = state.write();
    let id = store.next_run_id();
    let run = IngestRun {
        id,
        source: input.source,
        started_at: clock::normalize(started),
        finished_at: clock::normalize(finished),
        found: input.found,
        ingested: input.ingested,
        errors: input.errors,
    };
    store.runs.push(run);
    Ok(HttpResponse::Created().json(store.runs.last().expect("just pushed")))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(bulk).service(create_run);
}
