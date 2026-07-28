//! Operational endpoints: health and the SSE feed.

use std::convert::Infallible;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use bytes::Bytes;
use hyper::body::Frame;
use hyper::header::HeaderValue;
use hyper::{Response, header};
use serde::Serialize;
use tokio::time::{Interval, MissedTickBehavior, interval};

use crate::body::{self, ResBody};
use crate::store::AppState;
use crate::view;

#[derive(Serialize)]
struct Health {
    status: &'static str,
    articles: usize,
    uptime_seconds: u64,
}

pub fn health(state: &AppState) -> Response<ResBody> {
    let articles = state.articles.read().expect("articles lock").len();
    body::ok(&Health {
        status: "ok",
        articles,
        uptime_seconds: state.uptime_seconds(),
    })
}

/// One `article` event per second carrying the newest summary, with a comment
/// heartbeat after every fifth event.
///
/// There is no `Sse` response type to reach for, so the wire format is written
/// out and the body is a `Body` implementation driven straight by a Tokio
/// interval — no intermediate stream adapter and no per-connection task.
pub struct SseBody {
    state: AppState,
    ticker: Interval,
    emitted: u64,
}

impl SseBody {
    pub fn poll(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        ready!(self.ticker.poll_tick(context));

        // Scoped so the read guard is dropped before the next poll can suspend.
        let mut chunk = {
            let articles = self.state.articles.read().expect("articles lock");
            let payload = match articles.iter().next() {
                Some(article) => serde_json::to_string(&view::summary(&self.state, article))
                    .unwrap_or_else(|_| "null".to_owned()),
                None => "null".to_owned(),
            };
            let mut chunk = String::with_capacity(payload.len() + 32);
            chunk.push_str("event: article\ndata: ");
            chunk.push_str(&payload);
            chunk.push_str("\n\n");
            chunk
        };

        self.emitted += 1;
        if self.emitted % 5 == 0 {
            chunk.push_str(": heartbeat\n\n");
        }
        Poll::Ready(Some(Ok(Frame::data(Bytes::from(chunk)))))
    }
}

pub fn events(state: &AppState) -> Response<ResBody> {
    let mut ticker = interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let mut response = Response::new(ResBody::Sse(SseBody {
        state: state.clone(),
        ticker,
        emitted: 0,
    }));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}
