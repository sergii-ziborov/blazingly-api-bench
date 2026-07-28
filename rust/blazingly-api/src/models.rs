//! Request and response models.
//!
//! Every rule the contract states as a field constraint is declared here as an
//! attribute; the framework turns a violation into a 422 before the handler
//! runs. Rules that need the store (a slug that already exists, a category id
//! that must resolve) cannot be declared and live in the handlers.

use blazingly::prelude::*;
use blazingly::{
    ApiSchema, FieldDescriptor, ModelDescriptor, ValidationErrors, validation::ModelViolation,
};

// ---------------------------------------------------------------------------
// Shared views
// ---------------------------------------------------------------------------

/// The `{id, slug, name}` shape every nested reference uses.
#[api_model]
#[derive(Clone, Debug, Default)]
pub struct Ref {
    pub id: u32,
    pub slug: String,
    pub name: String,
}

/// The public projection of an article.
///
/// The store keeps one of these per article with its category, author and tag
/// references already resolved, so a listing clones a prepared value instead of
/// re-deriving one per item per request.
#[api_model]
#[derive(Clone, Debug)]
pub struct ArticleSummary {
    pub id: u32,
    pub slug: String,
    pub title: String,
    pub excerpt: String,
    pub lang: String,
    pub published_at: Option<String>,
    pub reading_minutes: u32,
    pub views: u64,
    pub category: Ref,
    pub author: Ref,
    pub tags: Vec<Ref>,
    pub cover_url: String,
}

/// The detail view is the summary plus three fields.
///
/// `#[api_model]` has no composition, so `#[serde(flatten)]` is what inlines
/// the summary on the wire. It costs a generated `ModelDescriptor` that
/// describes a nested `summary` object the response never contains.
#[api_model]
#[derive(Clone, Debug)]
pub struct ArticleDetail {
    #[serde(flatten)]
    pub summary: ArticleSummary,
    pub body: String,
    pub updated_at: String,
    pub related: Vec<ArticleSummary>,
}

/// Also the seed shape for categories and tags; the count is filled per request.
#[api_model]
#[derive(Clone, Debug)]
pub struct TaxonomyView {
    #[serde(flatten)]
    pub taxon: Ref,
    #[serde(default)]
    pub article_count: usize,
}

/// Also the seed shape for authors.
#[api_model]
#[derive(Clone, Debug)]
pub struct AuthorView {
    #[serde(flatten)]
    pub author: Ref,
    pub bio: String,
    #[serde(default)]
    pub article_count: usize,
}

/// Both the seed shape and the response shape; companies are never derived.
#[api_model]
#[derive(Clone, Debug)]
pub struct Company {
    pub id: u32,
    pub slug: String,
    pub name: String,
    pub industry: String,
    pub stage: String,
    pub founded_year: u32,
    pub employees: u32,
    pub total_funding_usd: i64,
    pub website: String,
}

// `#[api_model]` cannot be generic, so the paginated envelope is written once
// per item type.
#[api_model]
#[derive(Clone, Debug)]
pub struct ArticlePage {
    pub items: Vec<ArticleSummary>,
    pub page: u32,
    pub limit: u32,
    pub total: usize,
    pub pages: usize,
}

#[api_model]
#[derive(Clone, Debug)]
pub struct CompanyPage {
    pub items: Vec<Company>,
    pub page: u32,
    pub limit: u32,
    pub total: usize,
    pub pages: usize,
}

#[api_model]
#[derive(Clone, Debug)]
pub struct SearchResult {
    pub query: String,
    pub articles: Vec<ArticleSummary>,
    pub companies: Vec<Company>,
}

#[api_model]
#[derive(Clone, Debug)]
pub struct HealthView {
    pub status: String,
    pub articles: usize,
    pub uptime_seconds: u64,
}

#[api_model]
#[derive(Clone, Debug)]
pub struct CoverView {
    pub id: u32,
    pub cover_url: String,
    pub bytes: usize,
    pub content_type: String,
}

// ---------------------------------------------------------------------------
// Borrowed response views
// ---------------------------------------------------------------------------
//
// What the hot read paths actually put on the wire. The store already holds a
// finished `ArticleSummary` and `Company` per row, so a listing has nothing left
// to build: it borrows the rows and encodes them while it still holds the read
// guard, instead of cloning two hundred strings into an owned mirror that lives
// for the few microseconds until the body is written.
//
// `#[api_model]` cannot express a lifetime, so these are plain `Serialize`
// structs and the owned models above stay the documented schema — the operations
// still declare `ArticlePage`, `ArticleDetail`, `CompanyPage` and `SearchResult`
// through `PreparedJson<T>`, so OpenAPI and MCP are unchanged. Field for field
// each view mirrors its owned counterpart, so the bytes are the same too.

#[derive(serde::Serialize)]
pub struct PageView<'store, T> {
    pub items: Vec<&'store T>,
    pub page: u32,
    pub limit: u32,
    pub total: usize,
    pub pages: usize,
}

#[derive(serde::Serialize)]
pub struct DetailView<'store> {
    #[serde(flatten)]
    pub summary: &'store ArticleSummary,
    pub body: &'store str,
    pub updated_at: &'store str,
    pub related: Vec<&'store ArticleSummary>,
}

/// The echoed query borrows the request rather than the store; both outlive the
/// encode.
#[derive(serde::Serialize)]
pub struct SearchView<'store> {
    pub query: &'store str,
    pub articles: Vec<&'store ArticleSummary>,
    pub companies: Vec<&'store Company>,
}

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

#[api_model]
#[derive(Clone, Debug)]
pub struct ArticleQuery {
    #[minimum(1)]
    pub page: Option<u32>,
    #[minimum(1)]
    #[maximum(100)]
    pub limit: Option<u32>,
    pub category: Option<String>,
    pub tag: Option<String>,
    pub author: Option<String>,
    #[pattern("^(uk|ru|en)$")]
    pub lang: Option<String>,
    pub q: Option<String>,
}

#[api_model]
#[derive(Clone, Debug)]
pub struct CompanyQuery {
    #[minimum(1)]
    pub page: Option<u32>,
    #[minimum(1)]
    #[maximum(100)]
    pub limit: Option<u32>,
    pub industry: Option<String>,
    pub stage: Option<String>,
    pub min_funding: Option<i64>,
}

#[api_model]
#[derive(Clone, Debug)]
pub struct SearchQuery {
    #[min_length(2)]
    #[max_length(100)]
    pub q: String,
}

// ---------------------------------------------------------------------------
// Editorial writes
// ---------------------------------------------------------------------------

#[api_model]
#[derive(Clone, Debug)]
pub struct CreateArticle {
    #[min_length(8)]
    #[max_length(200)]
    pub title: String,
    #[min_length(3)]
    #[max_length(200)]
    #[pattern("^[a-z0-9]+(-[a-z0-9]+)*$")]
    pub slug: String,
    #[min_length(20)]
    #[max_length(500)]
    pub excerpt: String,
    #[min_length(50)]
    pub body: String,
    #[pattern("^(uk|ru|en)$")]
    pub lang: String,
    pub category_id: u32,
    pub author_id: u32,
    #[max_items(10)]
    #[unique_items]
    pub tag_ids: Vec<u32>,
}

#[api_model]
#[derive(Clone, Debug)]
pub struct PatchArticle {
    #[min_length(8)]
    #[max_length(200)]
    pub title: Option<String>,
    #[min_length(3)]
    #[max_length(200)]
    #[pattern("^[a-z0-9]+(-[a-z0-9]+)*$")]
    pub slug: Option<String>,
    #[min_length(20)]
    #[max_length(500)]
    pub excerpt: Option<String>,
    #[min_length(50)]
    pub body: Option<String>,
    #[pattern("^(uk|ru|en)$")]
    pub lang: Option<String>,
    pub category_id: Option<u32>,
    pub author_id: Option<u32>,
    #[max_items(10)]
    #[unique_items]
    pub tag_ids: Option<Vec<u32>>,
}

#[api_model]
#[derive(Clone, Debug)]
pub struct PublishRequest {
    #[validate_with(within_one_year)]
    pub published_at: DateTime,
}

/// A publication date more than a year out is rejected by the same declarative
/// pipeline that produced the field rules above.
pub fn within_one_year(value: &DateTime) -> Result<(), ValidationErrors> {
    const ONE_YEAR_SECONDS: i64 = 365 * 24 * 60 * 60;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    if value.as_inner().unix_timestamp() > now + ONE_YEAR_SECONDS {
        // The field path is supplied by the caller, so a custom validator
        // pushes an empty one or the merge produces `published_at.published_at`.
        let mut errors = ValidationErrors::new();
        errors.push(
            "",
            "too_far_ahead",
            "must not be more than one year in the future",
        );
        return Err(errors);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Ingestion
// ---------------------------------------------------------------------------

/// The bulk envelope carries `CreateArticle` items and implements `ApiModel` by
/// hand.
///
/// `#[api_model]` validates a `Vec<Model>` field automatically with no way to
/// opt out, so declaring `items: Vec<CreateArticle>` under the attribute would
/// fail the whole envelope with 422 the moment one item was invalid — the
/// opposite of what this endpoint has to do. Twelve hand-written lines buy back
/// per-item reporting; the alternative is a duplicate rule-free item struct and
/// a `From` impl that copies eight fields.
#[derive(Debug, serde::Deserialize)]
pub struct BulkRequest {
    pub items: Vec<CreateArticle>,
}

impl ApiModel for BulkRequest {
    fn model_descriptor() -> ModelDescriptor {
        ModelDescriptor::new(
            "BulkRequest",
            vec![FieldDescriptor::new(
                "items",
                true,
                <Vec<CreateArticle> as ApiSchema>::type_descriptor(),
                Vec::new(),
            )],
        )
    }

    fn validate(&self) -> Result<(), ValidationErrors> {
        Ok(())
    }
}

#[api_model]
#[derive(Clone, Debug)]
pub struct Violation {
    pub field: String,
    pub code: String,
    pub message: String,
}

#[api_model]
#[derive(Clone, Debug)]
pub struct BulkOutcome {
    pub index: usize,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<Violation>>,
}

#[api_model]
#[derive(Clone, Debug)]
pub struct BulkReport {
    pub accepted: usize,
    pub rejected: usize,
    pub results: Vec<BulkOutcome>,
}

/// One scrape run: the request body, the stored row and the response.
///
/// `DateTime` serializes through `Display`, which for `time::OffsetDateTime` is
/// `2026-04-01 12:00:00.0 +00:00:00` rather than RFC 3339, so both timestamps
/// carry an explicit serializer. Without it this endpoint would need a second
/// string-typed model and a field-by-field copy.
#[api_model(validate_with = finished_after_started)]
#[derive(Clone, Debug)]
pub struct IngestRun {
    /// Assigned by the store; a client cannot supply one.
    #[serde(default, skip_deserializing)]
    pub id: u64,
    #[min_length(1)]
    #[max_length(200)]
    pub source: String,
    #[serde(serialize_with = "as_rfc3339")]
    pub started_at: DateTime,
    #[serde(serialize_with = "as_rfc3339")]
    pub finished_at: DateTime,
    pub found: u32,
    pub ingested: u32,
    pub errors: u32,
}

pub fn finished_after_started(run: &IngestRun) -> Result<(), ModelViolation> {
    if run.finished_at.as_inner() < run.started_at.as_inner() {
        return Err(ModelViolation::field(
            "finished_at",
            "range",
            "must not precede started_at",
        ));
    }
    Ok(())
}

fn as_rfc3339<S: serde::Serializer>(value: &DateTime, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&crate::api::rfc3339(value.as_inner()))
}

// ---------------------------------------------------------------------------
// Domain errors
// ---------------------------------------------------------------------------

#[api_error]
#[derive(Clone, Debug)]
pub enum ApiError {
    #[status(404)]
    #[code("not_found")]
    #[message("The requested resource does not exist.")]
    NotFound,

    #[status(409)]
    #[code("already_published")]
    #[message("The article is already published.")]
    AlreadyPublished,

    #[status(413)]
    #[code("payload_too_large")]
    #[message("The cover image exceeds the 10 MiB limit.")]
    CoverTooLarge,

    #[status(415)]
    #[code("unsupported_media_type")]
    #[message("A cover image must be image/jpeg or image/png.")]
    UnsupportedCoverType,

    #[status(422)]
    #[code("validation_error")]
    #[message("The request failed validation.")]
    Invalid(Vec<Violation>),
}

impl Violation {
    pub fn new(field: &str, code: &str, message: impl Into<String>) -> Self {
        Self {
            field: field.to_owned(),
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

/// The framework's own `FieldViolation` is not an `#[api_model]`, so it cannot
/// appear inside a typed error payload and has to be copied across.
pub fn violations_of(errors: &ValidationErrors) -> Vec<Violation> {
    errors
        .violations()
        .iter()
        .map(|found| Violation::new(&found.field, &found.code, found.message.clone()))
        .collect()
}
