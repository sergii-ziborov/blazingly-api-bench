//! Transport-level policy: credential verification, the ingestion rate limit,
//! and the live article feed.

use blazingly::prelude::*;
use blazingly::{
    OperationDescriptor, SecurityRequirement, SecuritySchemeDescriptor, SecuritySchemeKind,
};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use crate::store::AppState;

const INGESTION_KEY: &str = "scraper-key";

pub fn security_schemes() -> [SecuritySchemeDescriptor; 2] {
    // The contract's editorial credential is a plain `Authorization: Bearer`
    // token, but an operation may only require scopes from a scheme declared as
    // OAuth2 — `Http { scheme: "bearer" }` rejects them at build time. Declaring
    // OAuth2 keeps the 401/403 split inside the security layer at the cost of an
    // OpenAPI document that calls a static bearer token an OAuth2 flow.
    let editorial = SecuritySchemeKind::OAuth2 {
        authorization_url: None,
        token_url: None,
        scopes: vec!["editor".to_owned(), "admin".to_owned()],
    };
    let ingestion = SecuritySchemeKind::ApiKey {
        location: blazingly::SecurityLocation::Header,
        name: "x-api-key".to_owned(),
    };
    [
        SecuritySchemeDescriptor::new("editorial", editorial),
        SecuritySchemeDescriptor::new("ingestion", ingestion),
    ]
}

/// Maps the two fixed editorial tokens onto scopes, so `#[security(...,
/// scopes = ["admin"])]` produces the contract's 401/403 split without any
/// role checking in a handler.
pub struct EditorialTokens;

impl TokenVerifier for EditorialTokens {
    fn verify_token(&self, token: &str) -> Result<VerifiedToken, AuthenticationError> {
        let scopes = match token {
            "editor-token" => vec!["editor".to_owned()],
            "admin-token" => vec!["editor".to_owned(), "admin".to_owned()],
            _ => return Err(AuthenticationError::Invalid("unknown editorial token")),
        };
        Ok(VerifiedToken {
            subject: Some(token.to_owned()),
            scopes,
            claims: blazingly::json::Value::Null,
        })
    }
}

/// The bundled `ApiKey` verifier refuses any secret shorter than 32 bytes, and
/// the contract fixes the key at `scraper-key`, so the scheme needs its own
/// verifier. It is the same eleven lines the bundled one runs.
pub struct ScraperApiKey;

impl CredentialVerifier for ScraperApiKey {
    fn verify(
        &self,
        context: &HttpRequestContext<'_>,
        _requirement: &SecurityRequirement,
        descriptor: &SecuritySchemeDescriptor,
    ) -> Result<AuthenticatedIdentity, AuthenticationError> {
        let SecuritySchemeKind::ApiKey { name, .. } = &descriptor.kind else {
            return Err(AuthenticationError::Internal(
                "API-key verifier attached to an incompatible scheme",
            ));
        };
        let supplied = context
            .request()
            .header_value(name, 0)
            .ok_or(AuthenticationError::Missing)?;
        let mismatch = supplied.len() != INGESTION_KEY.len()
            || supplied
                .bytes()
                .zip(INGESTION_KEY.bytes())
                .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
                != 0;
        if mismatch {
            return Err(AuthenticationError::Invalid("API key does not match"));
        }
        Ok(AuthenticatedIdentity {
            scheme: String::new(),
            subject: Some("scraper".to_owned()),
            scopes: Vec::new(),
            claims: blazingly::json::Value::Null,
        })
    }
}

// ---------------------------------------------------------------------------
// Rate limiting
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Bucket {
    tokens: f64,
    updated: Instant,
}

/// The bundled `MemoryRateLimitStore` is `!Sync`, and the multicore launcher
/// builds middleware per worker, so the default store would give each worker
/// its own bucket and admit `capacity x workers`. `RateLimitStore` is the seam
/// the framework provides for exactly this; a shared map behind it restores one
/// bucket per key for the whole process.
#[derive(Clone, Default)]
pub struct SharedRateLimitStore(Arc<Mutex<HashMap<String, Bucket>>>);

impl RateLimitStore for SharedRateLimitStore {
    fn consume(&self, key: &str, quota: RateLimitQuota, now: Instant) -> RateLimitDecision {
        let mut buckets = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let capacity = f64::from(quota.capacity());
        let bucket = buckets.entry(key.to_owned()).or_insert(Bucket {
            tokens: capacity,
            updated: now,
        });
        let elapsed = now.saturating_duration_since(bucket.updated).as_secs_f64();
        bucket.updated = now;
        bucket.tokens = (bucket.tokens + elapsed * quota.refill_per_second()).min(capacity);
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            RateLimitDecision::allow(bucket.tokens as u32)
        } else {
            let wait = (1.0 - bucket.tokens) / quota.refill_per_second().max(f64::MIN_POSITIVE);
            RateLimitDecision::deny(Duration::from_secs_f64(wait))
        }
    }

    fn tracked_keys(&self) -> usize {
        self.0
            .lock()
            .map(|buckets| buckets.len())
            .unwrap_or_default()
    }
}

/// Applies the framework's token-bucket `RateLimit` to the ingestion
/// operations only.
///
/// Middleware is registered for the whole application; there is no route or
/// scope predicate, so limiting one group of operations means gating the
/// framework layer on the matched operation id.
pub struct IngestRateLimit {
    inner: RateLimit,
}

impl IngestRateLimit {
    pub fn new(per_second: u32, store: SharedRateLimitStore) -> Self {
        let inner = RateLimit::keyed(per_second, Duration::from_secs(1), |context| {
            context
                .request()
                .header_value("x-api-key", 0)
                .map(str::to_owned)
        })
        .with_store(Rc::new(store));
        Self { inner }
    }
}

impl HttpMiddleware for IngestRateLimit {
    fn on_operation(
        &self,
        context: &mut HttpRequestContext<'_>,
        operation: &OperationDescriptor,
        _schemes: &[SecuritySchemeDescriptor],
    ) -> Option<Response> {
        if !operation.contract.id.as_str().starts_with("ingest.") {
            return None;
        }
        self.inner.on_request(context)
    }

    fn verifies_security(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Live feed
// ---------------------------------------------------------------------------

const TICK: Duration = Duration::from_secs(1);
const HEARTBEAT_EVERY: u64 = 5;

/// One `article` event per second plus a comment heartbeat every fifth event.
///
/// `StreamingBody` is a pull stream, so the pause between events has to come
/// from somewhere; the framework is runtime-neutral and ships no timer, so this
/// borrows the adapter's Compio timer directly.
pub struct ArticleFeed {
    state: AppState,
    emitted: u64,
    timer: Option<Pin<Box<dyn Future<Output = ()>>>>,
}

impl ArticleFeed {
    pub fn new(state: AppState) -> Sse {
        Sse::new(StreamingBody::new(Self {
            state,
            emitted: 0,
            timer: None,
        }))
    }
}

impl BodyStream for ArticleFeed {
    fn poll_next(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Vec<u8>, BodyStreamError>>> {
        let feed = self.get_mut();
        if feed.timer.is_none() {
            feed.timer = Some(Box::pin(compio::time::sleep(TICK)));
        }
        let elapsed = feed
            .timer
            .as_mut()
            .is_some_and(|timer| timer.as_mut().poll(context).is_ready());
        if !elapsed {
            return Poll::Pending;
        }
        feed.timer = None;
        feed.emitted += 1;

        let newest = {
            let articles = feed
                .state
                .articles
                .read()
                .unwrap_or_else(|error| error.into_inner());
            articles
                .ordered
                .first()
                .map(|article| article.summary.clone())
        };
        let mut chunk = Vec::with_capacity(1024);
        if let Some(summary) = newest
            && let Ok(event) = SseEvent::json(&summary)
            && let Ok(event) = event.with_event("article")
        {
            chunk.extend(event.encode());
        }
        if feed.emitted % HEARTBEAT_EVERY == 0 {
            chunk.extend(SseEvent::keep_alive("heartbeat").encode());
        }
        Poll::Ready(Some(Ok(chunk)))
    }
}
