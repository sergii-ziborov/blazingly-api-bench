//! Hyper-on-Tokio implementation of the API comparison contract in `SPEC.md`,
//! with no web framework.
//!
//! This is the floor the framework implementations are measured against: the
//! same contract, the same store, the same serialisation, and nothing between
//! the socket and the handler except hyper's HTTP/1 codec. Routing, query
//! decoding, credential checks, validation, error responses and SSE framing are
//! all in this crate rather than in a dependency.
//!
//! Port 3204. Tokio worker threads come from `BLAZINGLY_BENCH_WORKERS`
//! (default 4); the seed corpus from `BLAZINGLY_APIBENCH_SEED`, otherwise
//! `../../data/seed.json` relative to this crate.

mod admin;
mod body;
mod error;
mod ingest;
mod ops;
mod public;
mod query;
mod router;
mod store;
mod validate;
mod view;

use std::net::{Ipv4Addr, SocketAddr};

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

const PORT: u16 = 3204;
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
    let state = store::load()?;
    let articles = state.articles.read().expect("articles lock").len();
    let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, PORT));
    let listener = TcpListener::bind(address).await?;
    println!("tokio-api on http://{address} ({articles} articles, {workers} workers)");

    loop {
        // Socket options are left at the defaults on purpose: the framework
        // implementations do not touch them either, so the comparison stays a
        // comparison of what sits above the socket.
        let (stream, _peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            // A connection that died between the SYN and the accept must not
            // take the listener down with it.
            Err(error) if is_transient(&error) => continue,
            Err(error) => return Err(error.into()),
        };

        let state = state.clone();
        tokio::spawn(async move {
            let service = service_fn(move |request| router::handle(state.clone(), request));
            // The scenario client speaks HTTP/1.1 with keep-alive, which is
            // also all axum serves by default, so there is no protocol
            // auto-detection layer here.
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

fn is_transient(error: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    matches!(
        error.kind(),
        ErrorKind::ConnectionAborted
            | ErrorKind::ConnectionReset
            | ErrorKind::Interrupted
            | ErrorKind::InvalidInput
    )
}
