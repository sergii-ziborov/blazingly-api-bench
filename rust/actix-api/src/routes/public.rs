//! Unauthenticated read surface.

use std::collections::HashMap;

use actix_web::{HttpResponse, get, web};
use serde::Deserialize;

use crate::dto::{self, AuthorDetail, Facet, Page, SearchResults};
use crate::error::ApiError;
use crate::routes::{offset, page_limit};
use crate::store::{AppState, Article};
use crate::validation::{LANGS, STAGES};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    page: Option<u32>,
    limit: Option<u32>,
    category: Option<String>,
    tag: Option<String>,
    author: Option<String>,
    lang: Option<String>,
    q: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CompanyQuery {
    page: Option<u32>,
    limit: Option<u32>,
    industry: Option<String>,
    stage: Option<String>,
    min_funding: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    q: String,
}

fn check_lang(lang: Option<&str>) -> Result<(), ApiError> {
    match lang {
        Some(value) if !LANGS.contains(&value) => Err(ApiError::invalid_field(
            "lang",
            "one_of",
            format!("must be one of {}", LANGS.join(", ")),
        )),
        _ => Ok(()),
    }
}

fn matches_text(article: &Article, needle: &str) -> bool {
    article.title.to_lowercase().contains(needle)
        || article.excerpt.to_lowercase().contains(needle)
}

#[get("/articles")]
async fn list_articles(
    state: web::Data<AppState>,
    query: web::Query<ListQuery>,
) -> Result<HttpResponse, ApiError> {
    let (page, limit) = page_limit(query.page, query.limit)?;
    check_lang(query.lang.as_deref())?;

    let store = state.read();

    // An unknown facet slug is an empty page, not a 404, so `Some(None)` short
    // circuits instead of erroring.
    let category = query.category.as_deref().map(|slug| store.category_id_of(slug));
    let tag = query.tag.as_deref().map(|slug| store.tag_id_of(slug));
    let author = query.author.as_deref().map(|slug| store.author_by_slug(slug).map(|a| a.id));
    let unknown_facet = category == Some(None) || tag == Some(None) || author == Some(None);

    let needle = query.q.as_deref().map(str::to_lowercase);

    let matched: Vec<&Article> = if unknown_facet {
        Vec::new()
    } else {
        store
            .listing()
            .filter(|article| {
                category.flatten().is_none_or(|id| article.category_id == id)
                    && tag.flatten().is_none_or(|id| article.tag_ids.contains(&id))
                    && author.flatten().is_none_or(|id| article.author_id == id)
                    && query.lang.as_deref().is_none_or(|lang| article.lang == lang)
                    && needle.as_deref().is_none_or(|needle| matches_text(article, needle))
            })
            .collect()
    };

    let total = matched.len();
    let items = matched
        .into_iter()
        .skip(offset(page, limit))
        .take(limit as usize)
        .map(|article| dto::summary(&store, article))
        .collect();

    Ok(HttpResponse::Ok().json(Page::new(items, page, limit, total)))
}

#[get("/articles/{slug}")]
async fn get_article(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    let store = state.read();
    let article = store.article_by_slug(&path).ok_or(ApiError::NotFound("article"))?;
    Ok(HttpResponse::Ok().json(dto::detail(&store, article)))
}

#[get("/categories")]
async fn list_categories(state: web::Data<AppState>) -> HttpResponse {
    let store = state.read();
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for article in store.listing() {
        *counts.entry(article.category_id).or_default() += 1;
    }
    let items: Vec<Facet<'_>> = store
        .categories
        .iter()
        .map(|category| Facet {
            id: category.id,
            slug: &category.slug,
            name: &category.name,
            article_count: counts.get(&category.id).copied().unwrap_or(0),
        })
        .collect();
    HttpResponse::Ok().json(items)
}

#[get("/tags")]
async fn list_tags(state: web::Data<AppState>) -> HttpResponse {
    let store = state.read();
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for article in store.listing() {
        for id in &article.tag_ids {
            *counts.entry(*id).or_default() += 1;
        }
    }
    let items: Vec<Facet<'_>> = store
        .tags
        .iter()
        .map(|tag| Facet {
            id: tag.id,
            slug: &tag.slug,
            name: &tag.name,
            article_count: counts.get(&tag.id).copied().unwrap_or(0),
        })
        .collect();
    HttpResponse::Ok().json(items)
}

#[get("/authors/{slug}")]
async fn get_author(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    let store = state.read();
    let author = store.author_by_slug(&path).ok_or(ApiError::NotFound("author"))?;
    let article_count = store.listing().filter(|a| a.author_id == author.id).count();
    Ok(HttpResponse::Ok().json(AuthorDetail {
        id: author.id,
        slug: &author.slug,
        name: &author.name,
        bio: &author.bio,
        article_count,
    }))
}

#[get("/companies")]
async fn list_companies(
    state: web::Data<AppState>,
    query: web::Query<CompanyQuery>,
) -> Result<HttpResponse, ApiError> {
    let (page, limit) = page_limit(query.page, query.limit)?;
    if let Some(stage) = query.stage.as_deref()
        && !STAGES.contains(&stage)
    {
        return Err(ApiError::invalid_field(
            "stage",
            "one_of",
            format!("must be one of {}", STAGES.join(", ")),
        ));
    }

    let store = state.read();
    let matched: Vec<&crate::store::Company> = store
        .companies
        .iter()
        .filter(|company| {
            query.industry.as_deref().is_none_or(|value| company.industry == value)
                && query.stage.as_deref().is_none_or(|value| company.stage == value)
                && query.min_funding.is_none_or(|min| company.total_funding_usd >= min)
        })
        .collect();

    let total = matched.len();
    let items =
        matched.into_iter().skip(offset(page, limit)).take(limit as usize).collect::<Vec<_>>();

    Ok(HttpResponse::Ok().json(Page::new(items, page, limit, total)))
}

#[get("/search")]
async fn search(
    state: web::Data<AppState>,
    query: web::Query<SearchQuery>,
) -> Result<HttpResponse, ApiError> {
    let length = query.q.chars().count();
    if !(2..=100).contains(&length) {
        return Err(ApiError::invalid_field("q", "length", "must be 2 to 100 characters"));
    }
    let needle = query.q.to_lowercase();
    let store = state.read();

    let articles = store
        .listing()
        .filter(|article| matches_text(article, &needle))
        .take(10)
        .map(|article| dto::summary(&store, article))
        .collect();

    let companies = store
        .companies
        .iter()
        .filter(|company| {
            company.name.to_lowercase().contains(&needle)
                || company.slug.contains(&needle)
                || company.industry.contains(&needle)
        })
        .take(10)
        .collect();

    Ok(HttpResponse::Ok().json(SearchResults { query: &query.q, articles, companies }))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_articles)
        .service(get_article)
        .service(list_categories)
        .service(list_tags)
        .service(get_author)
        .service(list_companies)
        .service(search);
}
