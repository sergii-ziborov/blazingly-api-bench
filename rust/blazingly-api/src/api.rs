//! Every operation in the contract.

use blazingly::prelude::*;
use std::collections::HashMap;

use crate::infra::ArticleFeed;
use crate::models::{
    ApiError, ArticleDetail, ArticlePage, ArticleQuery, ArticleSummary, AuthorView, BulkOutcome,
    BulkReport, BulkRequest, CompanyPage, CompanyQuery, CompanyView, CoverView, CreateArticle,
    HealthView, IngestRunRequest, IngestRunView, PatchArticle, PublishRequest, Ref, SearchQuery,
    SearchResult, TaxonomyView, UploadHeaders, Violation, violations_of,
};
use crate::store::{AppState, Article, Articles, Company, Corpus, IngestRun, Taxon};

const DEFAULT_LIMIT: u32 = 20;
const MAX_COVER_BYTES: usize = 10 * 1024 * 1024;
const RELATED_LIMIT: usize = 3;
const SEARCH_LIMIT: usize = 10;

// ---------------------------------------------------------------------------
// Public reads
// ---------------------------------------------------------------------------

#[get("/articles", id = "articles.list", summary = "List articles")]
pub async fn list_articles(
    Query(query): Query<ArticleQuery>,
    state: AppState,
) -> Json<ArticlePage> {
    let (page, limit, offset) = page_bounds(query.page, query.limit);
    let corpus = &state.corpus;
    let articles = state.articles.read().unwrap_or_else(|e| e.into_inner());

    let mut unmatched = false;
    let category_id = resolve(
        query.category.as_deref(),
        |slug| corpus.category_id(slug),
        &mut unmatched,
    );
    let tag_id = resolve(
        query.tag.as_deref(),
        |slug| corpus.tag_id(slug),
        &mut unmatched,
    );
    let author_id = resolve(
        query.author.as_deref(),
        |slug| corpus.author_id(slug),
        &mut unmatched,
    );
    let lang = query.lang.as_deref();
    let needle = query.q.as_deref().map(Needle::new);

    if unmatched {
        return Json(ArticlePage {
            items: Vec::new(),
            page,
            limit,
            total: 0,
            pages: 0,
        });
    }

    let filtered = category_id.is_some()
        || tag_id.is_some()
        || author_id.is_some()
        || lang.is_some()
        || needle.is_some();
    if !filtered {
        let total = articles.len();
        let items = articles
            .ascending()
            .iter()
            .rev()
            .skip(offset)
            .take(limit as usize)
            .map(|article| summary_of(corpus, article))
            .collect();
        return Json(ArticlePage {
            items,
            page,
            limit,
            total,
            pages: page_count(total, limit),
        });
    }

    let mut total = 0_usize;
    let mut items = Vec::with_capacity(limit as usize);
    for article in articles.ascending().iter().rev() {
        if !article_matches(
            article,
            category_id,
            tag_id,
            author_id,
            lang,
            needle.as_ref(),
        ) {
            continue;
        }
        if total >= offset && items.len() < limit as usize {
            items.push(summary_of(corpus, article));
        }
        total += 1;
    }
    Json(ArticlePage {
        items,
        page,
        limit,
        total,
        pages: page_count(total, limit),
    })
}

#[get("/articles/{slug}", id = "articles.read", summary = "Read one article")]
pub async fn read_article(
    Path(slug): Path<String>,
    state: AppState,
) -> Result<Json<ArticleDetail>, ApiError> {
    let articles = state.articles.read().unwrap_or_else(|e| e.into_inner());
    let article = articles.by_slug(&slug).ok_or(ApiError::NotFound)?;
    Ok(Json(detail_of(&state.corpus, &articles, article)))
}

#[get("/categories", id = "categories.list", summary = "List categories")]
pub async fn list_categories(state: AppState) -> Json<Vec<TaxonomyView>> {
    let articles = state.articles.read().unwrap_or_else(|e| e.into_inner());
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for article in articles.ascending() {
        *counts.entry(article.category_id).or_default() += 1;
    }
    Json(taxonomy_views(&state.corpus.categories, &counts))
}

#[get("/tags", id = "tags.list", summary = "List tags")]
pub async fn list_tags(state: AppState) -> Json<Vec<TaxonomyView>> {
    let articles = state.articles.read().unwrap_or_else(|e| e.into_inner());
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for article in articles.ascending() {
        for tag_id in &article.tag_ids {
            *counts.entry(*tag_id).or_default() += 1;
        }
    }
    Json(taxonomy_views(&state.corpus.tags, &counts))
}

#[get("/authors/{slug}", id = "authors.read", summary = "Read one author")]
pub async fn read_author(
    Path(slug): Path<String>,
    state: AppState,
) -> Result<Json<AuthorView>, ApiError> {
    let author = state
        .corpus
        .author_id(&slug)
        .and_then(|id| state.corpus.author(id))
        .ok_or(ApiError::NotFound)?;
    let articles = state.articles.read().unwrap_or_else(|e| e.into_inner());
    let article_count = articles
        .ascending()
        .iter()
        .filter(|article| article.author_id == author.id)
        .count();
    Ok(Json(AuthorView {
        id: author.id,
        slug: author.slug.clone(),
        name: author.name.clone(),
        bio: author.bio.clone(),
        article_count,
    }))
}

#[get("/companies", id = "companies.list", summary = "List companies")]
pub async fn list_companies(
    Query(query): Query<CompanyQuery>,
    state: AppState,
) -> Json<CompanyPage> {
    let (page, limit, offset) = page_bounds(query.page, query.limit);
    let mut total = 0_usize;
    let mut items = Vec::with_capacity(limit as usize);
    for company in &state.corpus.companies {
        if !company_matches(company, &query) {
            continue;
        }
        if total >= offset && items.len() < limit as usize {
            items.push(company_view(company));
        }
        total += 1;
    }
    Json(CompanyPage {
        items,
        page,
        limit,
        total,
        pages: page_count(total, limit),
    })
}

#[get(
    "/search",
    id = "search.read",
    summary = "Search articles and companies"
)]
pub async fn search(Query(query): Query<SearchQuery>, state: AppState) -> Json<SearchResult> {
    let needle = Needle::new(&query.q);
    let articles = state.articles.read().unwrap_or_else(|e| e.into_inner());
    let matched_articles = articles
        .ascending()
        .iter()
        .rev()
        .filter(|article| needle.matches(&article.title) || needle.matches(&article.excerpt))
        .take(SEARCH_LIMIT)
        .map(|article| summary_of(&state.corpus, article))
        .collect();
    let matched_companies = state
        .corpus
        .companies
        .iter()
        .filter(|company| needle.matches(&company.name) || needle.matches(&company.industry))
        .take(SEARCH_LIMIT)
        .map(company_view)
        .collect();
    Json(SearchResult {
        query: query.q,
        articles: matched_articles,
        companies: matched_companies,
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
    let mut violations = reference_violations(&state.corpus, &input);
    if articles.contains_slug(&input.slug) {
        violations.push(Violation::new(
            "slug",
            "duplicate",
            "the slug already exists",
        ));
    }
    if !violations.is_empty() {
        return Err(ApiError::invalid(violations));
    }

    let id = articles.take_id();
    let stored = articles.insert(new_article(id, input));
    let location = format!("/articles/{}", stored.slug);
    let detail = detail_of(&state.corpus, &articles, &stored);
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
    let mut updated = {
        let existing = articles.by_id(id).ok_or(ApiError::NotFound)?;
        (**existing).clone()
    };
    let previous_slug = updated.slug.clone();

    let mut violations = Vec::new();
    if let Some(slug) = &input.slug
        && slug != &previous_slug
        && articles.contains_slug(slug)
    {
        violations.push(Violation::new(
            "slug",
            "duplicate",
            "the slug already exists",
        ));
    }
    if let Some(category_id) = input.category_id
        && state.corpus.category(category_id).is_none()
    {
        violations.push(Violation::new("category_id", "unknown", "no such category"));
    }
    if let Some(author_id) = input.author_id
        && state.corpus.author(author_id).is_none()
    {
        violations.push(Violation::new("author_id", "unknown", "no such author"));
    }
    if let Some(tag_ids) = &input.tag_ids {
        violations.extend(unknown_tag_violations(&state.corpus, tag_ids));
    }
    if !violations.is_empty() {
        return Err(ApiError::invalid(violations));
    }

    if let Some(title) = input.title {
        updated.title = title;
    }
    if let Some(slug) = input.slug {
        updated.slug = slug;
    }
    if let Some(excerpt) = input.excerpt {
        updated.excerpt = excerpt;
    }
    if let Some(body) = input.body {
        updated.reading_minutes = reading_minutes(&body);
        updated.body = body;
    }
    if let Some(lang) = input.lang {
        updated.lang = lang;
    }
    if let Some(category_id) = input.category_id {
        updated.category_id = category_id;
    }
    if let Some(author_id) = input.author_id {
        updated.author_id = author_id;
    }
    if let Some(tag_ids) = input.tag_ids {
        updated.tag_ids = tag_ids;
    }
    updated.updated_at = now_rfc3339();

    let stored = articles.replace(&previous_slug, updated);
    Ok(Json(detail_of(&state.corpus, &articles, &stored)))
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

/// The cover part is scanned as it arrives rather than decoded by
/// `Multipart<T>` or `File<UploadFile>`.
///
/// Two reasons, both recorded as friction: the native HTTP/1 adapter cannot
/// receive a buffered body larger than its 8 KiB connection buffer, and
/// `Multipart<T>` round-trips a binary part through `serde_json::Value`, which
/// for a 5 MiB image means a JSON array of five million numbers. `UploadBody`
/// is the extractor that both streams and stays out of `serde_json`.
#[post(
    "/admin/articles/{id}/cover",
    id = "admin.articles.cover",
    summary = "Replace an article cover"
)]
#[security("editorial", scopes = ["editor"])]
pub async fn upload_cover(
    Path(id): Path<u32>,
    Header(headers): Header<UploadHeaders>,
    body: UploadBody,
    state: AppState,
) -> Result<Json<CoverView>, ApiError> {
    let boundary = headers
        .content_type
        .as_deref()
        .and_then(multipart_boundary)
        .ok_or(ApiError::UnsupportedCoverType)?;
    let cover = scan_cover_part(body, &boundary).await?;
    if cover.field_name.as_deref() != Some("file") {
        return Err(ApiError::invalid(vec![Violation::new(
            "file",
            "missing",
            "the upload must carry one part named file",
        )]));
    }
    let media_type = cover.content_type.unwrap_or_default();
    if media_type != "image/jpeg" && media_type != "image/png" {
        return Err(ApiError::UnsupportedCoverType);
    }
    if cover.bytes > MAX_COVER_BYTES {
        return Err(ApiError::CoverTooLarge);
    }
    let articles = state.articles.read().unwrap_or_else(|e| e.into_inner());
    let article = articles.by_id(id).ok_or(ApiError::NotFound)?;
    Ok(Json(CoverView {
        id: article.id,
        cover_url: article.cover_url.clone(),
        bytes: cover.bytes,
        content_type: media_type,
    }))
}

#[post(
    "/admin/articles/{id}/publish",
    id = "admin.articles.publish",
    summary = "Publish an article"
)]
#[security("editorial", scopes = ["editor"])]
pub async fn publish_article(
    Path(id): Path<u32>,
    Json(input): Json<PublishRequest>,
    state: AppState,
) -> Result<Json<ArticleDetail>, ApiError> {
    let mut articles = state.articles.write().unwrap_or_else(|e| e.into_inner());
    let mut updated = {
        let existing = articles.by_id(id).ok_or(ApiError::NotFound)?;
        if existing.published_at.is_some() {
            return Err(ApiError::AlreadyPublished);
        }
        (**existing).clone()
    };
    let previous_slug = updated.slug.clone();
    updated.published_at = Some(rfc3339(input.published_at.as_inner()));
    updated.updated_at = now_rfc3339();
    let stored = articles.replace(&previous_slug, updated);
    Ok(Json(detail_of(&state.corpus, &articles, &stored)))
}

// ---------------------------------------------------------------------------
// Ingestion
// ---------------------------------------------------------------------------

/// The envelope is decoded by hand instead of by `Json<BulkRequest>`.
///
/// A 50-item batch is roughly 26 KiB and the native HTTP/1 adapter rejects any
/// buffered body larger than its 8 KiB connection read buffer with
/// `400 incomplete_body`. `UploadBody` takes the streaming dispatch path, which
/// reads correctly, at the cost of decoding and validating the envelope here
/// rather than declaring it.
#[post(
    "/ingest/articles/bulk",
    id = "ingest.articles.bulk",
    summary = "Ingest a batch of scraped articles"
)]
#[security("ingestion")]
pub async fn ingest_bulk(body: UploadBody, state: AppState) -> Result<Json<BulkReport>, ApiError> {
    let raw = read_to_end(body).await?;
    let request: BulkRequest = serde_json::from_slice(&raw).map_err(|error| {
        ApiError::invalid(vec![Violation::new(
            "items",
            "invalid_json",
            error.to_string(),
        )])
    })?;
    ApiModel::validate(&request).map_err(|errors| ApiError::invalid(violations_of(&errors)))?;

    let mut articles = state.articles.write().unwrap_or_else(|e| e.into_inner());
    let mut results = Vec::with_capacity(request.items.len());
    let mut accepted = 0_usize;

    for (index, item) in request.items.into_iter().enumerate() {
        let candidate = CreateArticle::from(item);
        // The same declarative rules the editorial endpoint enforces, run one
        // item at a time so a bad item rejects itself instead of the batch.
        if let Err(errors) = ApiModel::validate(&candidate) {
            results.push(rejected(index, violations_of(&errors)));
            continue;
        }
        let references = reference_violations(&state.corpus, &candidate);
        if !references.is_empty() {
            results.push(rejected(index, references));
            continue;
        }
        if articles.contains_slug(&candidate.slug) {
            results.push(BulkOutcome {
                index,
                status: "duplicate".to_owned(),
                id: None,
                slug: Some(candidate.slug),
                errors: None,
            });
            continue;
        }
        let id = articles.take_id();
        let stored = articles.insert(new_article(id, candidate));
        accepted += 1;
        results.push(BulkOutcome {
            index,
            status: "created".to_owned(),
            id: Some(stored.id),
            slug: Some(stored.slug.clone()),
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
pub async fn record_run(
    Json(input): Json<IngestRunRequest>,
    state: AppState,
) -> Created<IngestRunView> {
    let run = state
        .runs
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .record(IngestRun {
            id: 0,
            source: input.source,
            started_at: rfc3339(input.started_at.as_inner()),
            finished_at: rfc3339(input.finished_at.as_inner()),
            found: input.found,
            ingested: input.ingested,
            errors: input.errors,
        });
    Created(IngestRunView {
        id: run.id,
        source: run.source,
        started_at: run.started_at,
        finished_at: run.finished_at,
        found: run.found,
        ingested: run.ingested,
        errors: run.errors,
    })
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

#[get("/health", id = "health.read", summary = "Read service health")]
pub async fn health(state: AppState) -> Json<HealthView> {
    let articles = state.articles.read().unwrap_or_else(|e| e.into_inner());
    Json(HealthView {
        status: "ok".to_owned(),
        articles: articles.len(),
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
// Projections
// ---------------------------------------------------------------------------

pub fn summary_of(corpus: &Corpus, article: &Article) -> ArticleSummary {
    ArticleSummary {
        id: article.id,
        slug: article.slug.clone(),
        title: article.title.clone(),
        excerpt: article.excerpt.clone(),
        lang: article.lang.clone(),
        published_at: article.published_at.clone(),
        reading_minutes: article.reading_minutes,
        views: article.views,
        category: taxon_ref(corpus.category(article.category_id)),
        author: corpus
            .author(article.author_id)
            .map_or_else(missing_ref, |author| Ref {
                id: author.id,
                slug: author.slug.clone(),
                name: author.name.clone(),
            }),
        tags: article
            .tag_ids
            .iter()
            .filter_map(|id| corpus.tag(*id))
            .map(|tag| taxon_ref(Some(tag)))
            .collect(),
        cover_url: article.cover_url.clone(),
    }
}

fn detail_of(corpus: &Corpus, articles: &Articles, article: &Article) -> ArticleDetail {
    let related = articles
        .ascending()
        .iter()
        .rev()
        .filter(|other| other.category_id == article.category_id && other.id != article.id)
        .take(RELATED_LIMIT)
        .map(|other| summary_of(corpus, other))
        .collect();
    ArticleDetail::new(
        summary_of(corpus, article),
        article.body.clone(),
        article.updated_at.clone(),
        related,
    )
}

fn company_view(company: &Company) -> CompanyView {
    CompanyView {
        id: company.id,
        slug: company.slug.clone(),
        name: company.name.clone(),
        industry: company.industry.clone(),
        stage: company.stage.clone(),
        founded_year: company.founded_year,
        employees: company.employees,
        total_funding_usd: company.total_funding_usd,
        website: company.website.clone(),
    }
}

fn taxonomy_views(taxa: &[Taxon], counts: &HashMap<u32, usize>) -> Vec<TaxonomyView> {
    taxa.iter()
        .map(|taxon| TaxonomyView {
            id: taxon.id,
            slug: taxon.slug.clone(),
            name: taxon.name.clone(),
            article_count: counts.get(&taxon.id).copied().unwrap_or_default(),
        })
        .collect()
}

fn taxon_ref(taxon: Option<&Taxon>) -> Ref {
    taxon.map_or_else(missing_ref, |taxon| Ref {
        id: taxon.id,
        slug: taxon.slug.clone(),
        name: taxon.name.clone(),
    })
}

fn missing_ref() -> Ref {
    Ref {
        id: 0,
        slug: String::new(),
        name: String::new(),
    }
}

// ---------------------------------------------------------------------------
// Filtering and validation helpers
// ---------------------------------------------------------------------------

fn page_bounds(page: Option<u32>, limit: Option<u32>) -> (u32, u32, usize) {
    let page = page.unwrap_or(1);
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    let offset = (page as usize - 1) * limit as usize;
    (page, limit, offset)
}

fn page_count(total: usize, limit: u32) -> usize {
    total.div_ceil(limit as usize)
}

fn resolve(
    slug: Option<&str>,
    lookup: impl Fn(&str) -> Option<u32>,
    unmatched: &mut bool,
) -> Option<u32> {
    let slug = slug?;
    match lookup(slug) {
        Some(id) => Some(id),
        None => {
            *unmatched = true;
            None
        }
    }
}

fn article_matches(
    article: &Article,
    category_id: Option<u32>,
    tag_id: Option<u32>,
    author_id: Option<u32>,
    lang: Option<&str>,
    needle: Option<&Needle>,
) -> bool {
    if category_id.is_some_and(|id| article.category_id != id) {
        return false;
    }
    if tag_id.is_some_and(|id| !article.tag_ids.contains(&id)) {
        return false;
    }
    if author_id.is_some_and(|id| article.author_id != id) {
        return false;
    }
    if lang.is_some_and(|lang| article.lang != lang) {
        return false;
    }
    match needle {
        Some(needle) => needle.matches(&article.title) || needle.matches(&article.excerpt),
        None => true,
    }
}

fn company_matches(company: &Company, query: &CompanyQuery) -> bool {
    if query
        .industry
        .as_deref()
        .is_some_and(|industry| company.industry != industry)
    {
        return false;
    }
    if query
        .stage
        .as_deref()
        .is_some_and(|stage| company.stage != stage)
    {
        return false;
    }
    if query
        .min_funding
        .is_some_and(|minimum| company.total_funding_usd < minimum)
    {
        return false;
    }
    true
}

/// A case-insensitive substring matcher prepared once per request.
///
/// `search` scans the title and excerpt of every article, so the matcher does
/// one lowercase conversion per haystack character and never allocates: KMP
/// over the character stream rather than `to_lowercase().contains(..)`, which
/// would allocate two Strings for each of the thousand articles.
pub struct Needle {
    chars: Vec<char>,
    failure: Vec<usize>,
}

impl Needle {
    fn new(value: &str) -> Self {
        let chars: Vec<char> = value.chars().map(lower).collect();
        let mut failure = vec![0_usize; chars.len()];
        let mut length = 0;
        for index in 1..chars.len() {
            while length > 0 && chars[index] != chars[length] {
                length = failure[length - 1];
            }
            if chars[index] == chars[length] {
                length += 1;
            }
            failure[index] = length;
        }
        Self { chars, failure }
    }

    fn matches(&self, haystack: &str) -> bool {
        if self.chars.is_empty() {
            return true;
        }
        let mut length = 0_usize;
        for character in haystack.chars() {
            let character = lower(character);
            while length > 0 && character != self.chars[length] {
                length = self.failure[length - 1];
            }
            if character == self.chars[length] {
                length += 1;
                if length == self.chars.len() {
                    return true;
                }
            }
        }
        false
    }
}

fn lower(value: char) -> char {
    if value.is_ascii() {
        value.to_ascii_lowercase()
    } else {
        value.to_lowercase().next().unwrap_or(value)
    }
}

/// Referential rules the declarative attributes cannot express.
fn reference_violations(corpus: &Corpus, input: &CreateArticle) -> Vec<Violation> {
    let mut violations = Vec::new();
    if corpus.category(input.category_id).is_none() {
        violations.push(Violation::new("category_id", "unknown", "no such category"));
    }
    if corpus.author(input.author_id).is_none() {
        violations.push(Violation::new("author_id", "unknown", "no such author"));
    }
    violations.extend(unknown_tag_violations(corpus, &input.tag_ids));
    violations
}

fn unknown_tag_violations(corpus: &Corpus, tag_ids: &[u32]) -> Vec<Violation> {
    tag_ids
        .iter()
        .filter(|id| corpus.tag(**id).is_none())
        .map(|id| Violation::new("tag_ids", "unknown", format!("no such tag: {id}")))
        .collect()
}

// ---------------------------------------------------------------------------
// Streaming request bodies
// ---------------------------------------------------------------------------

const MAX_PART_HEADER_BYTES: usize = 8 * 1024;

async fn read_to_end(mut body: UploadBody) -> Result<Vec<u8>, ApiError> {
    let mut bytes = Vec::with_capacity(64 * 1024);
    while let Some(chunk) = body.next_chunk().await {
        bytes.extend_from_slice(&chunk.map_err(stream_failed)?);
    }
    Ok(bytes)
}

fn stream_failed(_error: blazingly::BodyStreamError) -> ApiError {
    ApiError::invalid(vec![Violation::new(
        "body",
        "stream_error",
        "the request body could not be read",
    )])
}

struct CoverPart {
    field_name: Option<String>,
    content_type: Option<String>,
    bytes: usize,
}

/// Counts the first multipart part without ever holding the whole upload.
///
/// Only the part headers and a delimiter-sized tail are kept, so peak memory
/// stays at one transport chunk regardless of the upload size.
async fn scan_cover_part(mut body: UploadBody, boundary: &str) -> Result<CoverPart, ApiError> {
    let delimiter = format!("\r\n--{boundary}").into_bytes();
    let mut carry: Vec<u8> = Vec::new();
    let mut part = CoverPart {
        field_name: None,
        content_type: None,
        bytes: 0,
    };
    let mut in_headers = true;
    let mut complete = false;

    while let Some(chunk) = body.next_chunk().await {
        carry.extend_from_slice(&chunk.map_err(stream_failed)?);
        loop {
            if in_headers {
                let Some(end) = find(&carry, b"\r\n\r\n") else {
                    if carry.len() > MAX_PART_HEADER_BYTES {
                        return Err(malformed_multipart());
                    }
                    break;
                };
                let (field_name, content_type) = parse_part_headers(&carry[..end]);
                part.field_name = field_name;
                part.content_type = content_type;
                carry.drain(..end + 4);
                in_headers = false;
                continue;
            }
            if let Some(position) = find(&carry, &delimiter) {
                part.bytes += position;
                complete = true;
                break;
            }
            let keep = (delimiter.len() - 1).min(carry.len());
            part.bytes += carry.len() - keep;
            carry.drain(..carry.len() - keep);
            break;
        }
        if complete || part.bytes > MAX_COVER_BYTES {
            break;
        }
    }

    if in_headers {
        return Err(malformed_multipart());
    }
    Ok(part)
}

fn malformed_multipart() -> ApiError {
    ApiError::invalid(vec![Violation::new(
        "file",
        "malformed",
        "the multipart body could not be parsed",
    )])
}

fn parse_part_headers(head: &[u8]) -> (Option<String>, Option<String>) {
    let mut field_name = None;
    let mut content_type = None;
    for line in head.split(|byte| *byte == b'\n') {
        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        let line = line.trim_end_matches('\r');
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-disposition") {
                field_name = disposition_name(value);
            } else if name.trim().eq_ignore_ascii_case("content-type") {
                content_type = Some(
                    value
                        .split(';')
                        .next()
                        .unwrap_or(value)
                        .trim()
                        .to_ascii_lowercase(),
                );
            }
        }
    }
    (field_name, content_type)
}

fn disposition_name(value: &str) -> Option<String> {
    for parameter in value.split(';') {
        let Some((key, raw)) = parameter.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("name") {
            return Some(raw.trim().trim_matches('"').to_owned());
        }
    }
    None
}

fn multipart_boundary(content_type: &str) -> Option<String> {
    let mut parts = content_type.split(';');
    if !parts
        .next()?
        .trim()
        .eq_ignore_ascii_case("multipart/form-data")
    {
        return None;
    }
    for parameter in parts {
        let Some((key, value)) = parameter.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("boundary") {
            let boundary = value.trim().trim_matches('"');
            if !boundary.is_empty() {
                return Some(boundary.to_owned());
            }
        }
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let first = *needle.first()?;
    let mut offset = 0;
    while offset + needle.len() <= haystack.len() {
        let Some(hit) = haystack[offset..].iter().position(|byte| *byte == first) else {
            return None;
        };
        let start = offset + hit;
        if start + needle.len() > haystack.len() {
            return None;
        }
        if &haystack[start..start + needle.len()] == needle {
            return Some(start);
        }
        offset = start + 1;
    }
    None
}

fn rejected(index: usize, errors: Vec<Violation>) -> BulkOutcome {
    BulkOutcome {
        index,
        status: "rejected".to_owned(),
        id: None,
        slug: None,
        errors: Some(errors),
    }
}

fn new_article(id: u32, input: CreateArticle) -> Article {
    Article {
        id,
        slug: input.slug,
        title: input.title,
        excerpt: input.excerpt,
        lang: input.lang,
        published_at: None,
        updated_at: now_rfc3339(),
        reading_minutes: reading_minutes(&input.body),
        views: 0,
        category_id: input.category_id,
        author_id: input.author_id,
        tag_ids: input.tag_ids,
        cover_url: format!("https://cdn.example/covers/{id:04}.jpg"),
        body: input.body,
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

/// `blazingly::validation::DateTime` serializes through `Display`, which for
/// `time::OffsetDateTime` is `2026-04-01 12:00:00.0 +00:00:00` rather than RFC
/// 3339, so a stored timestamp is encoded explicitly.
fn rfc3339(value: &time::OffsetDateTime) -> String {
    value
        .to_offset(time::UtcOffset::UTC)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
        .replace("+00:00", "Z")
}
