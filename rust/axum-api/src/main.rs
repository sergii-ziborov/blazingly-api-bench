//! Axum implementation of the API comparison contract in `SPEC.md`.
//!
//! Port 3202. Tokio worker threads come from `BLAZINGLY_BENCH_WORKERS`
//! (default 4); the seed corpus from `BLAZINGLY_APIBENCH_SEED`, otherwise
//! `../../data/seed.json` relative to this crate.

mod admin;
mod dto;
mod error;
mod extract;
mod ingest;
mod ops;
mod public;
mod state;
mod view;

use std::net::{Ipv4Addr, SocketAddr};

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{get, patch, post};
use tower_http::limit::RequestBodyLimitLayer;

use crate::state::AppState;

const PORT: u16 = 3202;
const DEFAULT_WORKERS: usize = 4;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workers = std::env::var("BLAZINGLY_BENCH_WORKERS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_WORKERS);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()?;

    runtime.block_on(serve(workers))
}

async fn serve(workers: usize) -> Result<(), Box<dyn std::error::Error>> {
    let state = state::load()?;
    let articles = state.articles.read().expect("articles lock").len();
    let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, PORT));
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("axum-api on http://{address} ({articles} articles, {workers} workers)");
    axum::serve(listener, app(state)).await?;
    Ok(())
}

fn app(state: AppState) -> Router {
    // Auth and the per-key rate limit apply to everything under /ingest, so
    // they live on the nested router rather than in each handler.
    let ingest = Router::new()
        .route("/articles/bulk", post(ingest::bulk))
        .route("/runs", post(ingest::create_run))
        .route_layer(middleware::from_fn_with_state(state.clone(), ingest::guard));

    Router::new()
        .route("/articles", get(public::list_articles))
        .route("/articles/{slug}", get(public::article_detail))
        .route("/categories", get(public::categories))
        .route("/tags", get(public::tags))
        .route("/authors/{slug}", get(public::author))
        .route("/companies", get(public::list_companies))
        .route("/search", get(public::search))
        .route("/health", get(ops::health))
        .route("/events", get(ops::events))
        .route("/admin/articles", post(admin::create_article))
        .route(
            "/admin/articles/{id}",
            patch(admin::update_article).delete(admin::delete_article),
        )
        .route(
            "/admin/articles/{id}/cover",
            // axum's 2 MiB default would reject the 5 MiB benchmark upload, so
            // it is replaced by a tower-http limit sized for the contract's
            // 10 MiB part plus multipart framing.
            post(admin::upload_cover).layer((
                DefaultBodyLimit::disable(),
                RequestBodyLimitLayer::new(dto::UPLOAD_HARD_LIMIT),
            )),
        )
        .route("/admin/articles/{id}/publish", post(admin::publish_article))
        .nest("/ingest", ingest)
        .with_state(state)
}
