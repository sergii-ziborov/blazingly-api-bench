//! The response body type, and the helpers that build responses out of it.
//!
//! A framework hands you an opaque `Body` that can be anything. Here there are
//! exactly two shapes — a buffer that is already complete, and the SSE ticker —
//! so they are an enum rather than a boxed trait object. That also makes
//! `size_hint` exact for the buffered case, which is what makes hyper emit
//! `Content-Length` instead of chunked framing.

use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use hyper::body::{Body, Frame, SizeHint};
use hyper::header::{HeaderName, HeaderValue};
use hyper::{Response, StatusCode, header};
use serde::Serialize;

use crate::ops::SseBody;

pub enum ResBody {
    /// A complete buffer, handed over in a single frame.
    Once(Option<Bytes>),
    Sse(SseBody),
}

impl ResBody {
    pub fn empty() -> Self {
        Self::Once(None)
    }

    pub fn full(bytes: Vec<u8>) -> Self {
        Self::Once(Some(Bytes::from(bytes)))
    }
}

impl Body for ResBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        match self.get_mut() {
            Self::Once(slot) => Poll::Ready(slot.take().map(|bytes| Ok(Frame::data(bytes)))),
            Self::Sse(stream) => stream.poll(context),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Self::Once(slot) => slot.is_none(),
            Self::Sse(_) => false,
        }
    }

    /// Exact for the buffered case so hyper can use `Content-Length`; the
    /// scenario client refuses chunked responses, and a framework would have
    /// got this right for us.
    fn size_hint(&self) -> SizeHint {
        match self {
            Self::Once(Some(bytes)) => SizeHint::with_exact(bytes.len() as u64),
            Self::Once(None) => SizeHint::with_exact(0),
            Self::Sse(_) => SizeHint::default(),
        }
    }
}

const APPLICATION_JSON: HeaderValue = HeaderValue::from_static("application/json");

pub fn empty(status: StatusCode) -> Response<ResBody> {
    let mut response = Response::new(ResBody::empty());
    *response.status_mut() = status;
    response
}

/// Serialises now, while any lock guard the value borrows from is still alive.
pub fn json<T: Serialize>(status: StatusCode, value: &T) -> Response<ResBody> {
    match serde_json::to_vec(value) {
        Ok(bytes) => raw_json(status, bytes),
        Err(_) => raw_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            br#"{"error":"internal","message":"response could not be serialized"}"#.to_vec(),
        ),
    }
}

pub fn raw_json(status: StatusCode, bytes: Vec<u8>) -> Response<ResBody> {
    let mut response = Response::new(ResBody::full(bytes));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, APPLICATION_JSON);
    response
}

pub fn ok<T: Serialize>(value: &T) -> Response<ResBody> {
    json(StatusCode::OK, value)
}

pub fn with_header(
    mut response: Response<ResBody>,
    name: HeaderName,
    value: &str,
) -> Response<ResBody> {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, value);
    }
    response
}
