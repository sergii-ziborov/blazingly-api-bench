//! Health and the SSE feed.

use std::convert::Infallible;
use std::time::Duration;

use actix_web::rt::time::sleep;
use actix_web::web::Bytes;
use actix_web::{HttpResponse, get, web};
use futures_util::stream;
use serde::Serialize;

use crate::dto;
use crate::store::AppState;

#[derive(Serialize)]
struct Health {
    status: &'static str,
    articles: usize,
    uptime_seconds: u64,
}

#[get("/health")]
async fn health(state: web::Data<AppState>) -> HttpResponse {
    let articles = state.read().article_count();
    HttpResponse::Ok().json(Health {
        status: "ok",
        articles,
        uptime_seconds: state.uptime_seconds(),
    })
}

/// Actix has no SSE helper in 4.x, but `HttpResponse::streaming` over a
/// `Stream<Item = Result<Bytes, _>>` is all it takes. Per-connection state is
/// one `Arc` clone and a `u64` tick counter.
#[get("/events")]
async fn events(state: web::Data<AppState>) -> HttpResponse {
    let feed = stream::unfold((state, 0u64), |(state, tick)| async move {
        sleep(Duration::from_secs(1)).await;
        let tick = tick + 1;

        let mut frame = String::with_capacity(512);
        {
            let store = state.read();
            if let Some(article) = store.newest() {
                frame.push_str("event: article\ndata: ");
                match serde_json::to_string(&dto::summary(&store, article)) {
                    Ok(payload) => frame.push_str(&payload),
                    Err(_) => frame.push_str("{}"),
                }
                frame.push_str("\n\n");
            }
        }
        if tick % 5 == 0 {
            frame.push_str(": heartbeat\n\n");
        }

        Some((Ok::<Bytes, Infallible>(Bytes::from(frame)), (state, tick)))
    });

    HttpResponse::Ok()
        .content_type("text/event-stream")
        .insert_header(("cache-control", "no-cache"))
        .streaming(feed)
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(health).service(events);
}
