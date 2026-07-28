//! Every operation in the contract.

use blazingly::prelude::*;
use serde::Serialize;
use std::collections::HashMap;

use crate::infra::ArticleFeed;
use crate::models::{
    ApiError, ArticleDetail, ArticlePage, ArticleQuery, ArticleSummary, AuthorView, BulkOutcome,
    BulkReport, BulkRequest, CompanyPage, CompanyQuery, CoverView, CreateArticle, DetailView,
    HealthView, IngestRun, PageView, PatchArticle, PublishRequest, SearchQuery, SearchResult,
    SearchView, TaxonomyView, Violation, violations_of,
};
use crate::store::{AppState, Article, Articles, RawArticle};

const DEFAULT_LIMIT: u32 = 20;
const MAX_COVER_BYTES: usize = 10 * 1024 * 1024;
const MAX_BULK_ITEMS: usize = 100;
const RELATED_LIMIT: usize = 3;
const SEARCH_LIMIT: usize = 10;

// ---------------------------------------------------------------------------
// Public reads
// ---------------------------------------------------------------------------

#[get("/articles", id = "articles.list", summary = "List articles")]
pub async fn list_articles(
    Query(query): Query<ArticleQuery>,
    state: AppState,
) -> PreparedJson<ArticlePage> {
    let (page, limit, offset) = page_bounds(query.page, query.limit);
    let corpus = &state.corpus;
    let mut unmatched = false;
    let category = resolve(&corpus.category_by_slug, &query.category, &mut unmatched);
    let tag = resolve(&corpus.tag_by_slug, &query.tag, &mut unmatched);
    let author = resolve(&corpus.author_by_slug, &query.author, &mut unmatched);
    let needle = query.q.as_deref().map(str::to_lowercase);
    let (lang, needle) = (query.lang.as_deref(), needle.as_deref());
    let narrowed = category.or(tag).or(author).is_some() || lang.is_some() || needle.is_some();

    // The guard is taken before `items`, which borrows the rows it collects, and
    // outlives it: the page is encoded before either is dropped.
    let articles = state.articles.read().unwrap_or_else(|e| e.into_inner());
    let mut items = Vec::with_capacity(limit as usize);
    let mut total = 0_usize;
    // An unknown slug yields an empty page rather than a 404, so it short
    // circuits the scan instead of filtering it.
    if !unmatched {
        for article in &articles.ordered {
            let summary = &article.summary;
            if category.is_some_and(|id| summary.category.id != id)
                || author.is_some_and(|id| summary.author.id != id)
                || tag.is_some_and(|id| !summary.tags.iter().any(|tag| tag.id == id))
                || lang.is_some_and(|lang| summary.lang != lang)
                || needle.is_some_and(|needle| !article.haystack.contains(needle))
            {
                continue;
            }
            total += 1;
            if total > offset && items.len() < limit as usize {
                items.push(summary);
            }
            // Nothing narrowed the corpus, so the total is arithmetic and the
            // scan stops at the end of the requested page rather than the end
            // of the thousand articles behind it.
            if !narrowed && items.len() == limit as usize {
                total = articles.ordered.len();
                break;
            }
        }
    }

    paged(items, page, limit, total)
}

#[get("/articles/{slug}", id = "articles.read", summary = "Read one article")]
pub async fn read_article(
    Path(slug): Path<String>,
    state: AppState,
) -> Result<PreparedJson<ArticleDetail>, ApiError> {
    let articles = state.articles.read().unwrap_or_else(|e| e.into_inner());
    let article = articles.by_slug.get(&slug).ok_or(ApiError::NotFound)?;
    Ok(encoded(&DetailView {
        summary: &article.summary,
        body: &article.body,
        updated_at: &article.updated_at,
        related: related_to(&articles, article).collect(),
    }))
}

#[get("/categories", id = "categories.list", summary = "List categories")]
pub async fn list_categories(state: AppState) -> Json<Vec<TaxonomyView>> {
    let articles = state.articles.read().unwrap_or_else(|e| e.into_inner());
    let mut counts = vec![0_usize; state.corpus.categories.len() + 1];
    for article in &articles.ordered {
        if let Some(slot) = counts.get_mut(article.summary.category.id as usize) {
            *slot += 1;
        }
    }
    Json(counted(&state.corpus.categories, &counts))
}

#[get("/tags", id = "tags.list", summary = "List tags")]
pub async fn list_tags(state: AppState) -> Json<Vec<TaxonomyView>> {
    let articles = state.articles.read().unwrap_or_else(|e| e.into_inner());
    let mut counts = vec![0_usize; state.corpus.tags.len() + 1];
    for article in &articles.ordered {
        for tag in &article.summary.tags {
            if let Some(slot) = counts.get_mut(tag.id as usize) {
                *slot += 1;
            }
        }
    }
    Json(counted(&state.corpus.tags, &counts))
}

#[get("/authors/{slug}", id = "authors.read", summary = "Read one author")]
pub async fn read_author(
    Path(slug): Path<String>,
    state: AppState,
) -> Result<Json<AuthorView>, ApiError> {
    let id = state.corpus.author_by_slug.get(&slug).copied();
    let mut author = id
        .and_then(|id| state.corpus.author(id))
        .ok_or(ApiError::NotFound)?
        .clone();
    let articles = state.articles.read().unwrap_or_else(|e| e.into_inner());
    author.article_count = articles
        .ordered
        .iter()
        .filter(|article| article.summary.author.id == author.author.id)
        .count();
    Ok(Json(author))
}

#[get("/companies", id = "companies.list", summary = "List companies")]
pub async fn list_companies(
    Query(query): Query<CompanyQuery>,
    state: AppState,
) -> PreparedJson<CompanyPage> {
    let (page, limit, offset) = page_bounds(query.page, query.limit);
    let (industry, stage) = (query.industry.as_deref(), query.stage.as_deref());
    // Two hundred companies never move, so the matched slice is walked twice —
    // once to size the page, once to fill it — rather than cloned once.
    let matched = state.corpus.companies.iter().filter(|company| {
        industry.is_none_or(|value| company.industry == value)
            && stage.is_none_or(|value| company.stage == value)
            && query
                .min_funding
                .is_none_or(|minimum| company.total_funding_usd >= minimum)
    });
    let items = matched.clone().skip(offset).take(limit as usize).collect();
    paged(items, page, limit, matched.count())
}

#[get(
    "/search",
    id = "search.read",
    summary = "Search articles and companies"
)]
pub async fn search(
    Query(query): Query<SearchQuery>,
    state: AppState,
) -> PreparedJson<SearchResult> {
    let needle = query.q.to_lowercase();
    let corpus = &state.corpus;
    let articles = state.articles.read().unwrap_or_else(|e| e.into_inner());
    encoded(&SearchView {
        articles: articles
            .ordered
            .iter()
            .filter(|article| article.haystack.contains(&needle))
            .take(SEARCH_LIMIT)
            .map(|article| &article.summary)
            .collect(),
        companies: corpus
            .companies
            .iter()
            .zip(&corpus.company_haystacks)
            .filter(|(_, haystack)| haystack.contains(&needle))
            .take(SEARCH_LIMIT)
            .map(|(company, _)| company)
            .collect(),
        query: &query.q,
    })
}

// ---------------------------------------------------------------------------
// Editorial writes
// ---------------------------------------------------------------------------

#[post(
    "/admin/articles",
    id = "admin.articles.create",
    summary = "Create an article"
)]
#[security("editorial", scopes = ["editor"])]
pub async fn create_article(
    Json(input): Json<CreateArticle>,
    state: AppState,
) -> Result<WithHeaders<Created<ArticleDetail>>, ApiError> {
    let mut articles = state.articles.write().unwrap_or_else(|e| e.into_inner());
    let mut violations = reference_violations(&state, &input);
    if articles.contains_slug(&input.slug) {
        violations.push(Violation::new("slug", "duplicate", "the slug exists"));
    }
    if !violations.is_empty() {
        return Err(ApiError::Invalid(violations));
    }

    let id = articles.take_id();
    let stored = articles.insert(state.corpus.assemble(draft(id, input)));
    let location = format!("/articles/{}", stored.summary.slug);
    let detail = detail_of(&articles, &stored);
    Ok(Created(detail).header("location", location))
}

#[patch(
    "/admin/articles/{id}",
    id = "admin.articles.update",
    summary = "Update an article"
)]
#[security("editorial", scopes = ["editor"])]
pub async fn update_article(
    Path(id): Path<u32>,
    Json(input): Json<PatchArticle>,
    state: AppState,
) -> Result<Json<ArticleDetail>, ApiError> {
    let mut articles = state.articles.write().unwrap_or_else(|e| e.into_inner());
    let existing = articles.by_id.get(&id).ok_or(ApiError::NotFound)?;
    // Merging the patch onto the stored article first means one referential
    // check and one rebuild, rather than eight conditional field assignments
    // that each have to remember to refresh a derived field.
    let previous = existing.summary.clone();
    let merged = CreateArticle {
        title: input.title.unwrap_or(previous.title),
        slug: input.slug.unwrap_or_else(|| previous.slug.clone()),
        excerpt: input.excerpt.unwrap_or(previous.excerpt),
        body: input.body.unwrap_or_else(|| existing.body.clone()),
        lang: input.lang.unwrap_or(previous.lang),
        category_id: input.category_id.unwrap_or(previous.category.id),
        author_id: input.author_id.unwrap_or(previous.author.id),
        tag_ids: input
            .tag_ids
            .unwrap_or_else(|| previous.tags.iter().map(|tag| tag.id).collect()),
    };

    let mut violations = reference_violations(&state, &merged);
    if merged.slug != previous.slug && articles.contains_slug(&merged.slug) {
        violations.push(Violation::new("slug", "duplicate", "the slug exists"));
    }
    if !violations.is_empty() {
        return Err(ApiError::Invalid(violations));
    }

    let mut raw = draft(id, merged);
    raw.published_at = previous.published_at;
    raw.views = previous.views;
    raw.cover_url = previous.cover_url;
    let stored = articles.replace(&previous.slug, state.corpus.assemble(raw));
    Ok(Json(detail_of(&articles, &stored)))
}

#[delete(
    "/admin/articles/{id}",
    id = "admin.articles.delete",
    summary = "Delete an article"
)]
#[security("editorial", scopes = ["admin"])]
pub async fn delete_article(Path(id): Path<u32>, state: AppState) -> Result<NoContent, ApiError> {
    let mut articles = state.articles.write().unwrap_or_else(|e| e.into_inner());
    if articles.remove(id) {
        Ok(NoContent)
    } else {
        Err(ApiError::NotFound)
    }
}

/// `File<UploadFile>` buffers the whole part before the handler runs, which the
/// contract permits and the peak-RSS column is there to expose. There is no
/// streaming multipart extractor to reach for instead.
#[post(
    "/admin/articles/{id}/cover",
    id = "admin.articles.cover",
    summary = "Replace a cover"
)]
#[security("editorial", scopes = ["editor"])]
pub async fn upload_cover(
    Path(id): Path<u32>,
    File(file): File<UploadFile>,
    state: AppState,
) -> Result<Json<CoverView>, ApiError> {
    let content_type = file.content_type.unwrap_or_default();
    if content_type != "image/jpeg" && content_type != "image/png" {
        return Err(ApiError::UnsupportedCoverType);
    }
    if file.bytes.len() > MAX_COVER_BYTES {
        return Err(ApiError::CoverTooLarge);
    }
    let articles = state.articles.read().unwrap_or_else(|e| e.into_inner());
    let article = articles.by_id.get(&id).ok_or(ApiError::NotFound)?;
    Ok(Json(CoverView {
        id: article.summary.id,
        cover_url: article.summary.cover_url.clone(),
        bytes: file.bytes.len(),
        content_type,
    }))
}

#[post(
    "/admin/articles/{id}/publish",
    id = "admin.articles.publish",
    summary = "Publish"
)]
#[security("editorial", scopes = ["editor"])]
pub async fn publish_article(
    Path(id): Path<u32>,
    Json(input): Json<PublishRequest>,
    state: AppState,
) -> Result<Json<ArticleDetail>, ApiError> {
    let mut articles = state.articles.write().unwrap_or_else(|e| e.into_inner());
    let existing = articles.by_id.get(&id).ok_or(ApiError::NotFound)?;
    if existing.summary.published_at.is_some() {
        return Err(ApiError::AlreadyPublished);
    }
    let mut updated = (**existing).clone();
    let previous_slug = updated.summary.slug.clone();
    updated.summary.published_at = Some(rfc3339(input.published_at.as_inner()));
    updated.updated_at = now_rfc3339();
    let stored = articles.replace(&previous_slug, updated);
    Ok(Json(detail_of(&articles, &stored)))
}

// ---------------------------------------------------------------------------
// Ingestion
// ---------------------------------------------------------------------------

#[post(
    "/ingest/articles/bulk",
    id = "ingest.articles.bulk",
    summary = "Ingest a batch"
)]
#[security("ingestion")]
pub async fn ingest_bulk(
    Json(request): Json<BulkRequest>,
    state: AppState,
) -> Result<Json<BulkReport>, ApiError> {
    if !(1..=MAX_BULK_ITEMS).contains(&request.items.len()) {
        let size = Violation::new("items", "size", "a batch carries 1 to 100 items");
        return Err(ApiError::Invalid(vec![size]));
    }

    let mut articles = state.articles.write().unwrap_or_else(|e| e.into_inner());
    let mut results = Vec::with_capacity(request.items.len());
    let mut accepted = 0_usize;

    for (index, candidate) in request.items.into_iter().enumerate() {
        // The same declarative rules the editorial endpoint enforces, run one
        // item at a time so a bad item rejects itself instead of the batch.
        let rejection = match ApiModel::validate(&candidate) {
            Err(errors) => violations_of(&errors),
            Ok(()) => reference_violations(&state, &candidate),
        };
        if !rejection.is_empty() {
            results.push(outcome(index, "rejected", None, Some(rejection)));
            continue;
        }
        if articles.contains_slug(&candidate.slug) {
            results.push(outcome(index, "duplicate", Some(candidate.slug), None));
            continue;
        }
        let id = articles.take_id();
        let stored = articles.insert(state.corpus.assemble(draft(id, candidate)));
        accepted += 1;
        results.push(BulkOutcome {
            index,
            status: "created".to_owned(),
            id: Some(stored.summary.id),
            slug: Some(stored.summary.slug.clone()),
            errors: None,
        });
    }

    Ok(Json(BulkReport {
        accepted,
        rejected: results.len() - accepted,
        results,
    }))
}

#[post(
    "/ingest/runs",
    id = "ingest.runs.create",
    summary = "Record a scrape run"
)]
#[security("ingestion")]
pub async fn record_run(Json(input): Json<IngestRun>, state: AppState) -> Created<IngestRun> {
    let mut runs = state.runs.write().unwrap_or_else(|e| e.into_inner());
    Created(runs.record(input))
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

#[get("/health", id = "health.read", summary = "Read service health")]
pub async fn health(state: AppState) -> Json<HealthView> {
    let articles = state.articles.read().unwrap_or_else(|e| e.into_inner());
    Json(HealthView {
        status: "ok".to_owned(),
        articles: articles.ordered.len(),
        uptime_seconds: state.uptime_seconds(),
    })
}

#[get(
    "/events",
    id = "events.stream",
    summary = "Stream newly published articles"
)]
pub async fn events(state: AppState) -> Sse {
    ArticleFeed::new(state)
}

// ---------------------------------------------------------------------------
// Projections and helpers
// ---------------------------------------------------------------------------

/// Encodes a borrowed view as the body of `S`, the schema the operation
/// documents. The views are plain structs of strings and integers, so the error
/// arm is unreachable; it costs a byte string rather than a panic.
fn encoded<S, V: Serialize + ?Sized>(view: &V) -> PreparedJson<S> {
    PreparedJson::encode(view).unwrap_or_else(|_| PreparedJson::from_bytes(b"null".to_vec()))
}

/// The paginated envelope both listings answer with.
fn paged<S, T: Serialize>(items: Vec<&T>, page: u32, limit: u32, total: usize) -> PreparedJson<S> {
    encoded(&PageView {
        items,
        page,
        limit,
        total,
        pages: total.div_ceil(limit as usize),
    })
}

/// The related-articles rule, shared by the borrowed detail view and the owned
/// one the write paths still return.
fn related_to<'store>(
    articles: &'store Articles,
    article: &Article,
) -> impl Iterator<Item = &'store ArticleSummary> {
    let (category, id) = (article.summary.category.id, article.summary.id);
    articles
        .ordered
        .iter()
        .map(|other| &other.summary)
        .filter(move |other| other.category.id == category && other.id != id)
        .take(RELATED_LIMIT)
}

fn detail_of(articles: &Articles, article: &Article) -> ArticleDetail {
    ArticleDetail {
        related: related_to(articles, article).cloned().collect(),
        summary: article.summary.clone(),
        body: article.body.clone(),
        updated_at: article.updated_at.clone(),
    }
}

fn counted(views: &[TaxonomyView], counts: &[usize]) -> Vec<TaxonomyView> {
    views
        .iter()
        .map(|view| TaxonomyView {
            article_count: counts[view.taxon.id as usize],
            taxon: view.taxon.clone(),
        })
        .collect()
}

fn page_bounds(page: Option<u32>, limit: Option<u32>) -> (u32, u32, usize) {
    let page = page.unwrap_or(1);
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    (page, limit, (page as usize - 1) * limit as usize)
}

/// An unknown slug is not an error, so it is reported through `unmatched`
/// rather than by returning one.
fn resolve(
    index: &HashMap<String, u32>,
    slug: &Option<String>,
    unmatched: &mut bool,
) -> Option<u32> {
    let resolved = index.get(slug.as_ref()?).copied();
    *unmatched |= resolved.is_none();
    resolved
}

/// Referential rules the declarative attributes cannot express.
fn reference_violations(state: &AppState, input: &CreateArticle) -> Vec<Violation> {
    let corpus = &state.corpus;
    let mut found = Vec::new();
    if corpus.category(input.category_id).is_none() {
        found.push(Violation::new("category_id", "unknown", "no such category"));
    }
    if corpus.author(input.author_id).is_none() {
        found.push(Violation::new("author_id", "unknown", "no such author"));
    }
    for id in input.tag_ids.iter().filter(|id| corpus.tag(**id).is_none()) {
        found.push(Violation::new(
            "tag_ids",
            "unknown",
            format!("no such tag: {id}"),
        ));
    }
    found
}

fn outcome(
    index: usize,
    status: &str,
    slug: Option<String>,
    errors: Option<Vec<Violation>>,
) -> BulkOutcome {
    BulkOutcome {
        index,
        status: status.to_owned(),
        id: None,
        slug,
        errors,
    }
}

fn draft(id: u32, input: CreateArticle) -> RawArticle {
    RawArticle {
        id,
        reading_minutes: reading_minutes(&input.body),
        published_at: None,
        updated_at: now_rfc3339(),
        views: 0,
        cover_url: format!("https://cdn.example/covers/{id:04}.jpg"),
        input,
    }
}

fn reading_minutes(body: &str) -> u32 {
    let words = body.split_whitespace().count();
    u32::try_from(words.div_ceil(200))
        .unwrap_or(u32::MAX)
        .max(1)
}

fn now_rfc3339() -> String {
    rfc3339(&time::OffsetDateTime::now_utc())
}

pub fn rfc3339(value: &time::OffsetDateTime) -> String {
    value
        .to_offset(time::UtcOffset::UTC)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
        .replace("+00:00", "Z")
}
