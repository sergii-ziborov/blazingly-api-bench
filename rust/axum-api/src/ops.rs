//! Operational endpoints: health and the SSE feed.

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::response::sse::{Event, Sse};
use axum::response::IntoResponse;
use serde::Serialize;
use tokio::time::MissedTickBehavior;

use crate::state::AppState;
use crate::view;

#[derive(Serialize)]
pub struct Health {
    status: &'static str,
    articles: usize,
    uptime_seconds: u64,
}

pub async fn health(State(state): State<AppState>) -> Json<Health> {
    let articles = state.articles.read().expect("articles lock").len();
    Json(Health {
        status: "ok",
        articles,
        uptime_seconds: state.uptime_seconds(),
    })
}

/// One `article` event per second carrying the newest summary, with a comment
/// heartbeat after every fifth event.
pub async fn events(State(state): State<AppState>) -> impl IntoResponse {
    let stream = async_stream::stream! {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut emitted: u64 = 0;

        loop {
            ticker.tick().await;
            // Scoped so the read guard is never held across an await point.
            let payload = {
                let articles = state.articles.read().expect("articles lock");
                match articles.iter().next() {
                    Some(article) => serde_json::to_string(&view::summary(&state, article))
                        .unwrap_or_else(|_| "null".to_owned()),
                    None => "null".to_owned(),
                }
            };

            yield Ok::<Event, std::convert::Infallible>(
                Event::default().event("article").data(payload),
            );
            emitted += 1;
            if emitted % 5 == 0 {
                yield Ok(Event::default().comment("heartbeat"));
            }
        }
    };

    Sse::new(stream)
}
