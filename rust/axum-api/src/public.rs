//! Public read endpoints.

use axum::extract::{Path, State};
use axum::response::Response;

use crate::dto::{ArticleQuery, CompanyQuery, SearchQuery, lang_is_valid, stage_is_valid};
use crate::error::ApiError;
use crate::extract::ValidQuery;
use crate::state::AppState;
use crate::view::{self, AuthorProfile, Facet, Page, SearchResults, Summary};

const SEARCH_CAP: usize = 10;

/// Slug filters that match nothing yield an empty page rather than a 404, so an
/// unknown slug short-circuits the scan entirely.
#[derive(Clone, Copy)]
enum Filter {
    Any,
    Id(u32),
    None,
}

impl Filter {
    fn resolve(slug: Option<&String>, lookup: &std::collections::HashMap<String, u32>) -> Self {
        match slug {
            None => Self::Any,
            Some(slug) => match lookup.get(slug.as_str()) {
                Some(id) => Self::Id(*id),
                None => Self::None,
            },
        }
    }
}

fn empty_page(page: u32, limit: u32) -> Page<Summary<'static>> {
    Page {
        items: Vec::new(),
        page,
        limit,
        total: 0,
        pages: 0,
    }
}

pub async fn list_articles(
    State(state): State<AppState>,
    ValidQuery(query): ValidQuery<ArticleQuery>,
) -> Result<Response, ApiError> {
    if let Some(lang) = &query.lang
        && !lang_is_valid(lang)
    {
        return Err(ApiError::field("lang", "one_of", "must be one of uk, ru, en"));
    }

    let category = Filter::resolve(query.category.as_ref(), &state.category_by_slug);
    let tag = Filter::resolve(query.tag.as_ref(), &state.tag_by_slug);
    let author = Filter::resolve(query.author.as_ref(), &state.author_by_slug);
    if matches!(category, Filter::None) || matches!(tag, Filter::None) || matches!(author, Filter::None)
    {
        return view::ok(&empty_page(query.page, query.limit));
    }

    let needle = query.q.as_ref().map(|value| value.to_lowercase());
    let limit = query.limit as usize;
    let skip = (query.page as usize - 1) * limit;

    let articles = state.articles.read().expect("articles lock");
    let mut items = Vec::new();
    let mut total = 0_usize;
    for article in articles.iter() {
        if let Filter::Id(id) = category
            && article.category_id != id
        {
            continue;
        }
        if let Filter::Id(id) = tag
            && !article.tag_ids.contains(&id)
        {
            continue;
        }
        if let Filter::Id(id) = author
            && article.author_id != id
        {
            continue;
        }
        if let Some(lang) = &query.lang
            && article.lang != *lang
        {
            continue;
        }
        if let Some(needle) = &needle
            && !article.haystack.contains(needle.as_str())
        {
            continue;
        }
        if total >= skip && items.len() < limit {
            items.push(view::summary(&state, article));
        }
        total += 1;
    }

    view::ok(&Page {
        items,
        page: query.page,
        limit: query.limit,
        total,
        pages: total.div_ceil(limit),
    })
}

pub async fn article_detail(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Response, ApiError> {
    let articles = state.articles.read().expect("articles lock");
    let article = articles.find(&slug).ok_or(ApiError::NotFound("article"))?;
    view::ok(&view::detail(&state, &articles, article))
}

pub async fn categories(State(state): State<AppState>) -> Result<Response, ApiError> {
    let articles = state.articles.read().expect("articles lock");
    let mut counts = vec![0_usize; state.categories.len() + 1];
    for article in articles.iter() {
        if let Some(slot) = counts.get_mut(article.category_id as usize) {
            *slot += 1;
        }
    }
    let items: Vec<Facet<'_>> = state
        .categories
        .iter()
        .map(|category| Facet {
            id: category.id,
            slug: &category.slug,
            name: &category.name,
            article_count: counts[category.id as usize],
        })
        .collect();
    view::ok(&items)
}

pub async fn tags(State(state): State<AppState>) -> Result<Response, ApiError> {
    let articles = state.articles.read().expect("articles lock");
    let mut counts = vec![0_usize; state.tags.len() + 1];
    for article in articles.iter() {
        for id in &article.tag_ids {
            if let Some(slot) = counts.get_mut(*id as usize) {
                *slot += 1;
            }
        }
    }
    let items: Vec<Facet<'_>> = state
        .tags
        .iter()
        .map(|tag| Facet {
            id: tag.id,
            slug: &tag.slug,
            name: &tag.name,
            article_count: counts[tag.id as usize],
        })
        .collect();
    view::ok(&items)
}

pub async fn author(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Response, ApiError> {
    let id = state
        .author_by_slug
        .get(&slug)
        .copied()
        .ok_or(ApiError::NotFound("author"))?;
    let author = state.author(id).ok_or(ApiError::NotFound("author"))?;
    let articles = state.articles.read().expect("articles lock");
    let article_count = articles
        .iter()
        .filter(|article| article.author_id == id)
        .count();
    view::ok(&AuthorProfile {
        id: author.id,
        slug: &author.slug,
        name: &author.name,
        bio: &author.bio,
        article_count,
    })
}

pub async fn list_companies(
    State(state): State<AppState>,
    ValidQuery(query): ValidQuery<CompanyQuery>,
) -> Result<Response, ApiError> {
    if let Some(stage) = &query.stage
        && !stage_is_valid(stage)
    {
        return Err(ApiError::field(
            "stage",
            "one_of",
            "must be one of seed, series_a, series_b, growth",
        ));
    }

    let limit = query.limit as usize;
    let skip = (query.page as usize - 1) * limit;
    let mut items = Vec::new();
    let mut total = 0_usize;
    for company in &state.companies {
        if let Some(industry) = &query.industry
            && company.industry != *industry
        {
            continue;
        }
        if let Some(stage) = &query.stage
            && company.stage != *stage
        {
            continue;
        }
        if let Some(min_funding) = query.min_funding
            && company.total_funding_usd < min_funding
        {
            continue;
        }
        if total >= skip && items.len() < limit {
            items.push(company);
        }
        total += 1;
    }

    view::ok(&Page {
        items,
        page: query.page,
        limit: query.limit,
        total,
        pages: total.div_ceil(limit),
    })
}

pub async fn search(
    State(state): State<AppState>,
    ValidQuery(query): ValidQuery<SearchQuery>,
) -> Result<Response, ApiError> {
    let needle = query.q.to_lowercase();
    let articles = state.articles.read().expect("articles lock");

    let mut matched = Vec::with_capacity(SEARCH_CAP);
    for article in articles.iter() {
        if article.haystack.contains(needle.as_str()) {
            matched.push(view::summary(&state, article));
            if matched.len() == SEARCH_CAP {
                break;
            }
        }
    }

    let companies = state
        .companies
        .iter()
        .filter(|company| company.haystack.contains(needle.as_str()))
        .take(SEARCH_CAP)
        .collect();

    view::ok(&SearchResults {
        query: &query.q,
        articles: matched,
        companies,
    })
}
