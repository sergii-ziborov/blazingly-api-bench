//! The Blazingly implementation of the apibench contract.
//!
//! One plugin holds the shared corpus and the two security schemes; the
//! multicore native server builds one compiled application per worker.

mod api;
mod infra;
mod models;
mod store;

use blazingly::native::MulticoreServer;
use blazingly::prelude::*;
use std::num::NonZeroUsize;
use std::rc::Rc;

use api::*;
use infra::{
    EditorialTokens, IngestRateLimit, ScraperApiKey, SharedRateLimitStore, security_schemes,
};
use store::AppState;

const PORT: u16 = 3201;
/// Comfortably above the contract's 10 MiB cover limit so the handler, not the
/// wire layer, decides when an upload is too large.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

fn build_app(state: AppState) -> ExecutableApp {
    let plugin = security_schemes()
        .into_iter()
        .fold(Plugin::new("apibench"), Plugin::security_scheme)
        .provide(Provider::value(state))
        .routes(routes![
            list_articles,
            read_article,
            list_categories,
            list_tags,
            read_author,
            list_companies,
            search,
            create_article,
            update_article,
            delete_article,
            upload_cover,
            publish_article,
            ingest_bulk,
            record_run,
            health,
            events,
        ]);
    ExecutableApp::from_plugin(plugin).expect("the operation graph compiles")
}

fn main() -> std::io::Result<()> {
    let state = AppState::load()?;
    let workers =
        NonZeroUsize::new(env_number("BLAZINGLY_BENCH_WORKERS", 4)).unwrap_or(NonZeroUsize::MIN);
    let ingest_per_second = env_number("APIBENCH_INGEST_RPS", 100) as u32;
    let buckets = SharedRateLimitStore::default();

    MulticoreServer::new(workers, move || build_app(state.clone()))
        .with_max_body_bytes(MAX_BODY_BYTES)
        .with_middleware_factory(move || {
            let security = Security::new()
                .verifier("editorial", OAuth2Bearer::new(EditorialTokens))
                .verifier("ingestion", ScraperApiKey);
            let throttle = IngestRateLimit::new(ingest_per_second, buckets.clone());
            vec![
                Rc::new(security) as Rc<dyn HttpMiddleware>,
                Rc::new(throttle) as Rc<dyn HttpMiddleware>,
            ]
        })
        .serve(("127.0.0.1", PORT))
}

fn env_number(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}
