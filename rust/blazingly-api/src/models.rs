//! Request and response models.
//!
//! Every rule the contract states as a field constraint is declared here as an
//! attribute; the framework turns a violation into a 422 before the handler
//! runs. Rules that need the store (a slug that already exists, a category id
//! that must resolve) cannot be declared and live in the handlers.

use blazingly::prelude::*;
use blazingly::{ValidationErrors, validation::ModelViolation};

// ---------------------------------------------------------------------------
// Shared views
// ---------------------------------------------------------------------------

#[api_model]
#[derive(Clone, Debug)]
pub struct Ref {
    pub id: u32,
    pub slug: String,
    pub name: String,
}

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

// `#[api_model]` has no flattening or composition, so a detail view restates
// every summary field instead of embedding one.
#[api_model]
#[derive(Clone, Debug)]
pub struct ArticleDetail {
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
    pub body: String,
    pub updated_at: String,
    pub related: Vec<ArticleSummary>,
}

impl ArticleDetail {
    pub fn new(
        summary: ArticleSummary,
        body: String,
        updated_at: String,
        related: Vec<ArticleSummary>,
    ) -> Self {
        Self {
            id: summary.id,
            slug: summary.slug,
            title: summary.title,
            excerpt: summary.excerpt,
            lang: summary.lang,
            published_at: summary.published_at,
            reading_minutes: summary.reading_minutes,
            views: summary.views,
            category: summary.category,
            author: summary.author,
            tags: summary.tags,
            cover_url: summary.cover_url,
            body,
            updated_at,
            related,
        }
    }
}

#[api_model]
#[derive(Clone, Debug)]
pub struct TaxonomyView {
    pub id: u32,
    pub slug: String,
    pub name: String,
    pub article_count: usize,
}

#[api_model]
#[derive(Clone, Debug)]
pub struct AuthorView {
    pub id: u32,
    pub slug: String,
    pub name: String,
    pub bio: String,
    pub article_count: usize,
}

#[api_model]
#[derive(Clone, Debug)]
pub struct CompanyView {
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
    pub items: Vec<CompanyView>,
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
    pub companies: Vec<CompanyView>,
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

/// `Header<String>` derives the header name from the argument identifier and
/// maps `_` to `-`. The native *streaming* request view compares the raw name
/// instead, so an operation that also takes `UploadBody` never sees
/// `content-type` through a scalar `Header<String>`. A one-field model with an
/// explicit alias restores it, because the model path does consult aliases.
#[api_model]
#[derive(Clone, Debug)]
pub struct UploadHeaders {
    #[alias("content-type")]
    pub content_type: Option<String>,
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

/// The bulk item deliberately carries no declarative rules.
///
/// `#[api_model]` validates a `Vec<Model>` field automatically and there is no
/// way to opt out, so declaring `items: Vec<CreateArticle>` would fail the
/// whole envelope with 422 the moment one item was invalid — the opposite of
/// what this endpoint has to do. The items are converted and validated one at
/// a time in the handler instead.
#[api_model]
#[derive(Clone, Debug)]
pub struct BulkItem {
    pub title: String,
    pub slug: String,
    pub excerpt: String,
    pub body: String,
    pub lang: String,
    pub category_id: u32,
    pub author_id: u32,
    pub tag_ids: Vec<u32>,
}

impl From<BulkItem> for CreateArticle {
    fn from(item: BulkItem) -> Self {
        Self {
            title: item.title,
            slug: item.slug,
            excerpt: item.excerpt,
            body: item.body,
            lang: item.lang,
            category_id: item.category_id,
            author_id: item.author_id,
            tag_ids: item.tag_ids,
        }
    }
}

#[api_model]
#[derive(Clone, Debug)]
pub struct BulkRequest {
    #[min_items(1)]
    #[max_items(100)]
    pub items: Vec<BulkItem>,
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

#[api_model(validate_with = finished_after_started)]
#[derive(Clone, Debug)]
pub struct IngestRunRequest {
    #[min_length(1)]
    #[max_length(200)]
    pub source: String,
    pub started_at: DateTime,
    pub finished_at: DateTime,
    pub found: u32,
    pub ingested: u32,
    pub errors: u32,
}

pub fn finished_after_started(run: &IngestRunRequest) -> Result<(), ModelViolation> {
    if run.finished_at.as_inner() < run.started_at.as_inner() {
        return Err(ModelViolation::field(
            "finished_at",
            "range",
            "must not precede started_at",
        ));
    }
    Ok(())
}

#[api_model]
#[derive(Clone, Debug)]
pub struct IngestRunView {
    pub id: u64,
    pub source: String,
    pub started_at: String,
    pub finished_at: String,
    pub found: u32,
    pub ingested: u32,
    pub errors: u32,
}

// ---------------------------------------------------------------------------
// Domain errors
// ---------------------------------------------------------------------------

#[api_model]
#[derive(Clone, Debug)]
pub struct ViolationDetails {
    pub violations: Vec<Violation>,
}

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
    Invalid(ViolationDetails),
}

impl ApiError {
    pub fn invalid(violations: Vec<Violation>) -> Self {
        Self::Invalid(ViolationDetails { violations })
    }
}

impl Violation {
    pub fn new(
        field: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            field: field.into(),
            code: code.into(),
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
        .map(|violation| {
            Violation::new(
                violation.field.clone(),
                violation.code.clone(),
                violation.message.clone(),
            )
        })
        .collect()
}
