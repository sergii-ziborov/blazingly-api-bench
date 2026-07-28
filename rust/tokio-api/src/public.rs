//! Public read endpoints.
//!
//! Every handler here is synchronous: nothing awaits, so the store's read guard
//! can be held across the whole request including serialisation. The router
//! calls them directly rather than through a boxed handler trait.

use std::collections::HashMap;

use hyper::Response;

use crate::body::{self, ResBody};
use crate::error::ApiError;
use crate::query::{self, parse_number};
use crate::store::AppState;
use crate::validate::{lang_is_valid, stage_is_valid};
use crate::view::{self, AuthorProfile, Facet, Page, SearchResults, Summary};

const SEARCH_CAP: usize = 10;
const DEFAULT_PAGE: u32 = 1;
const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 100;

/// Slug filters that match nothing yield an empty page rather than a 404, so an
/// unknown slug short-circuits the scan entirely.
#[derive(Clone, Copy)]
enum Filter {
    Any,
    Id(u32),
    None,
}

impl Filter {
    fn resolve(slug: Option<&str>, lookup: &HashMap<String, u32>) -> Self {
        match slug {
            None => Self::Any,
            Some(slug) => match lookup.get(slug) {
                Some(id) => Self::Id(*id),
                None => Self::None,
            },
        }
    }
}

fn check_page_limit(page: u32, limit: u32) -> Result<(), ApiError> {
    if page < DEFAULT_PAGE {
        return Err(ApiError::field("page", "min_value", "must be at least 1"));
    }
    if limit < 1 {
        return Err(ApiError::field("limit", "min_value", "must be at least 1"));
    }
    if limit > MAX_LIMIT {
        return Err(ApiError::field("limit", "max_value", "must be at most 100"));
    }
    Ok(())
}

pub fn list_articles(state: &AppState, raw_query: &str) -> Result<Response<ResBody>, ApiError> {
    let mut page = DEFAULT_PAGE;
    let mut limit = DEFAULT_LIMIT;
    let mut category = None;
    let mut tag = None;
    let mut author = None;
    let mut lang = None;
    let mut needle = None;

    for (key, value) in query::pairs(raw_query) {
        match key.as_ref() {
            "page" => page = parse_number("page", &value)?,
            "limit" => limit = parse_number("limit", &value)?,
            "category" => category = Some(value),
            "tag" => tag = Some(value),
            "author" => author = Some(value),
            "lang" => lang = Some(value),
            "q" => needle = Some(value.to_lowercase()),
            _ => {}
        }
    }

    check_page_limit(page, limit)?;
    if let Some(lang) = &lang
        && !lang_is_valid(lang)
    {
        return Err(ApiError::field("lang", "one_of", "must be one of uk, ru, en"));
    }

    let category = Filter::resolve(category.as_deref(), &state.category_by_slug);
    let tag = Filter::resolve(tag.as_deref(), &state.tag_by_slug);
    let author = Filter::resolve(author.as_deref(), &state.author_by_slug);
    if matches!(category, Filter::None)
        || matches!(tag, Filter::None)
        || matches!(author, Filter::None)
    {
        let items: Vec<Summary<'_>> = Vec::new();
        return Ok(body::ok(&Page {
            items,
            page,
            limit,
            total: 0,
            pages: 0,
        }));
    }

    let window = limit as usize;
    let skip = (page as usize - 1) * window;

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
        if let Some(lang) = &lang
            && article.lang != *lang
        {
            continue;
        }
        if let Some(needle) = &needle
            && !article.haystack.contains(needle.as_str())
        {
            continue;
        }
        if total >= skip && items.len() < window {
            items.push(view::summary(state, article));
        }
        total += 1;
    }

    Ok(body::ok(&Page {
        items,
        page,
        limit,
        total,
        pages: total.div_ceil(window),
    }))
}

pub fn article_detail(state: &AppState, slug: &str) -> Result<Response<ResBody>, ApiError> {
    let articles = state.articles.read().expect("articles lock");
    let article = articles.find(slug).ok_or(ApiError::NotFound("article"))?;
    Ok(body::ok(&view::detail(state, &articles, article)))
}

pub fn categories(state: &AppState) -> Response<ResBody> {
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
    body::ok(&items)
}

pub fn tags(state: &AppState) -> Response<ResBody> {
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
    body::ok(&items)
}

pub fn author(state: &AppState, slug: &str) -> Result<Response<ResBody>, ApiError> {
    let id = state
        .author_by_slug
        .get(slug)
        .copied()
        .ok_or(ApiError::NotFound("author"))?;
    let author = state.author(id).ok_or(ApiError::NotFound("author"))?;
    let articles = state.articles.read().expect("articles lock");
    let article_count = articles
        .iter()
        .filter(|article| article.author_id == id)
        .count();
    Ok(body::ok(&AuthorProfile {
        id: author.id,
        slug: &author.slug,
        name: &author.name,
        bio: &author.bio,
        article_count,
    }))
}

pub fn list_companies(state: &AppState, raw_query: &str) -> Result<Response<ResBody>, ApiError> {
    let mut page = DEFAULT_PAGE;
    let mut limit = DEFAULT_LIMIT;
    let mut industry = None;
    let mut stage = None;
    let mut min_funding = None;

    for (key, value) in query::pairs(raw_query) {
        match key.as_ref() {
            "page" => page = parse_number("page", &value)?,
            "limit" => limit = parse_number("limit", &value)?,
            "industry" => industry = Some(value),
            "stage" => stage = Some(value),
            "min_funding" => min_funding = Some(parse_number::<u64>("min_funding", &value)?),
            _ => {}
        }
    }

    check_page_limit(page, limit)?;
    if let Some(stage) = &stage
        && !stage_is_valid(stage)
    {
        return Err(ApiError::field(
            "stage",
            "one_of",
            "must be one of seed, series_a, series_b, growth",
        ));
    }

    let window = limit as usize;
    let skip = (page as usize - 1) * window;
    let mut items = Vec::new();
    let mut total = 0_usize;
    for company in &state.companies {
        if let Some(industry) = &industry
            && company.industry != **industry
        {
            continue;
        }
        if let Some(stage) = &stage
            && company.stage != **stage
        {
            continue;
        }
        if let Some(min_funding) = min_funding
            && company.total_funding_usd < min_funding
        {
            continue;
        }
        if total >= skip && items.len() < window {
            items.push(company);
        }
        total += 1;
    }

    Ok(body::ok(&Page {
        items,
        page,
        limit,
        total,
        pages: total.div_ceil(window),
    }))
}

pub fn search(state: &AppState, raw_query: &str) -> Result<Response<ResBody>, ApiError> {
    let mut term = None;
    for (key, value) in query::pairs(raw_query) {
        if key == "q" {
            term = Some(value);
        }
    }
    let Some(term) = term else {
        return Err(ApiError::malformed("Failed to deserialize query string: missing field `q`"));
    };

    let length = term.chars().count();
    if length < 2 {
        return Err(ApiError::field(
            "q",
            "min_length",
            "must be at least 2 characters",
        ));
    }
    if length > 100 {
        return Err(ApiError::field(
            "q",
            "max_length",
            "must be at most 100 characters",
        ));
    }

    let needle = term.to_lowercase();
    let articles = state.articles.read().expect("articles lock");

    let mut matched = Vec::with_capacity(SEARCH_CAP);
    for article in articles.iter() {
        if article.haystack.contains(needle.as_str()) {
            matched.push(view::summary(state, article));
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

    Ok(body::ok(&SearchResults {
        query: &term,
        articles: matched,
        companies,
    }))
}
