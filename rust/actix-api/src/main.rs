//! Actix Web implementation of the apibench contract (port 3203).

mod auth;
mod clock;
mod dto;
mod error;
mod routes;
mod store;
mod validation;

use std::path::PathBuf;

use actix_web::middleware::from_fn;
use actix_web::{App, HttpRequest, HttpServer, web};

use crate::error::ApiError;
use crate::store::{AppState, Store};

const PORT: u16 = 3203;
const DEFAULT_WORKERS: usize = 4;
const REQUESTS_PER_SECOND: u32 = 100;
const JSON_LIMIT: usize = 16 * 1024 * 1024;

fn seed_path() -> PathBuf {
    match std::env::var_os("BLAZINGLY_APIBENCH_SEED") {
        Some(path) => PathBuf::from(path),
        // Resolved against the crate, not the working directory, so the server
        // starts the same way from anywhere.
        None => PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/seed.json")),
    }
}

fn workers() -> usize {
    std::env::var("BLAZINGLY_BENCH_WORKERS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_WORKERS)
}

/// Actix defaults body- and query-deserialisation failures to 400; the contract
/// wants 422 for both, so every extractor gets its error handler rewired.
fn json_error(err: actix_web::error::JsonPayloadError, _: &HttpRequest) -> actix_web::Error {
    match err {
        actix_web::error::JsonPayloadError::OverflowKnownLength { .. }
        | actix_web::error::JsonPayloadError::Overflow { .. } => {
            ApiError::PayloadTooLarge(JSON_LIMIT).into()
        }
        actix_web::error::JsonPayloadError::ContentType => {
            ApiError::UnsupportedMediaType("expected application/json".to_owned()).into()
        }
        other => ApiError::invalid_field("body", "malformed", other.to_string()).into(),
    }
}

fn query_error(err: actix_web::error::QueryPayloadError, _: &HttpRequest) -> actix_web::Error {
    ApiError::invalid_field("query", "malformed", err.to_string()).into()
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let path = seed_path();
    let store = Store::load(&path).map_err(|err| {
        std::io::Error::other(format!("failed to load seed {}: {err}", path.display()))
    })?;
    let articles = store.article_count();
    // The contract fixes the ingestion budget at 100 req/s per key. The
    // benchmark harness raises it for the bulk scenario, because otherwise that
    // sample measures the rate limiter rather than validation. All four
    // implementations read the same variable so the scenario stays comparable.
    let requests_per_second = std::env::var("APIBENCH_INGEST_RPS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(REQUESTS_PER_SECOND);
    let state = web::Data::new(AppState::new(store, requests_per_second));
    let workers = workers();

    println!("actix-api listening on http://127.0.0.1:{PORT} ({articles} articles, {workers} workers)");

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .app_data(web::JsonConfig::default().limit(JSON_LIMIT).error_handler(json_error))
            .app_data(web::QueryConfig::default().error_handler(query_error))
            .configure(routes::public::configure)
            .configure(routes::ops::configure)
            .service(web::scope("/admin").configure(routes::admin::configure))
            .service(
                web::scope("/ingest")
                    .wrap(from_fn(routes::ingest::guard))
                    .configure(routes::ingest::configure),
            )
    })
    .workers(workers)
    .bind(("127.0.0.1", PORT))?
    .run()
    .await
}
