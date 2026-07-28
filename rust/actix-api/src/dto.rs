//! Response shapes.
//!
//! They borrow from the store rather than cloning: every handler already holds
//! the read guard while `HttpResponse::json` serialises, so the whole listing
//! path is allocation-free apart from the output buffer.

use serde::Serialize;

use crate::store::{Article, Store};

#[derive(Serialize)]
pub struct Ref<'a> {
    pub id: u32,
    pub slug: &'a str,
    pub name: &'a str,
}

#[derive(Serialize)]
pub struct ArticleSummary<'a> {
    pub id: u64,
    pub slug: &'a str,
    pub title: &'a str,
    pub excerpt: &'a str,
    pub lang: &'a str,
    pub published_at: Option<&'a str>,
    pub reading_minutes: u32,
    pub views: u64,
    pub category: Option<Ref<'a>>,
    pub author: Option<Ref<'a>>,
    pub tags: Vec<Ref<'a>>,
    pub cover_url: &'a str,
}

/// Deliberately not `#[serde(flatten)] summary: ArticleSummary` — flatten makes
/// serde buffer the inner struct through a `Map`, which would put a serde cost
/// into the `detail` benchmark that has nothing to do with the framework.
#[derive(Serialize)]
pub struct ArticleDetail<'a> {
    pub id: u64,
    pub slug: &'a str,
    pub title: &'a str,
    pub excerpt: &'a str,
    pub lang: &'a str,
    pub published_at: Option<&'a str>,
    pub reading_minutes: u32,
    pub views: u64,
    pub category: Option<Ref<'a>>,
    pub author: Option<Ref<'a>>,
    pub tags: Vec<Ref<'a>>,
    pub cover_url: &'a str,
    pub body: &'a str,
    pub updated_at: &'a str,
    pub related: Vec<ArticleSummary<'a>>,
}

#[derive(Serialize)]
pub struct Facet<'a> {
    pub id: u32,
    pub slug: &'a str,
    pub name: &'a str,
    pub article_count: usize,
}

#[derive(Serialize)]
pub struct AuthorDetail<'a> {
    pub id: u32,
    pub slug: &'a str,
    pub name: &'a str,
    pub bio: &'a str,
    pub article_count: usize,
}

#[derive(Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: u32,
    pub limit: u32,
    pub total: usize,
    pub pages: u32,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, page: u32, limit: u32, total: usize) -> Self {
        let pages = total.div_ceil(limit as usize) as u32;
        Self { items, page, limit, total, pages }
    }
}

#[derive(Serialize)]
pub struct SearchResults<'a> {
    pub query: &'a str,
    pub articles: Vec<ArticleSummary<'a>>,
    pub companies: Vec<&'a crate::store::Company>,
}

pub fn summary<'a>(store: &'a Store, article: &'a Article) -> ArticleSummary<'a> {
    ArticleSummary {
        id: article.id,
        slug: &article.slug,
        title: &article.title,
        excerpt: &article.excerpt,
        lang: &article.lang,
        published_at: article.published_at.as_deref(),
        reading_minutes: article.reading_minutes,
        views: article.views,
        category: store
            .category(article.category_id)
            .map(|c| Ref { id: c.id, slug: &c.slug, name: &c.name }),
        author: store
            .author(article.author_id)
            .map(|a| Ref { id: a.id, slug: &a.slug, name: &a.name }),
        tags: article
            .tag_ids
            .iter()
            .filter_map(|&id| store.tag(id))
            .map(|t| Ref { id: t.id, slug: &t.slug, name: &t.name })
            .collect(),
        cover_url: &article.cover_url,
    }
}

pub fn detail<'a>(store: &'a Store, article: &'a Article) -> ArticleDetail<'a> {
    let related = store
        .listing()
        .filter(|other| other.category_id == article.category_id && other.id != article.id)
        .take(3)
        .map(|other| summary(store, other))
        .collect();

    let s = summary(store, article);
    ArticleDetail {
        id: s.id,
        slug: s.slug,
        title: s.title,
        excerpt: s.excerpt,
        lang: s.lang,
        published_at: s.published_at,
        reading_minutes: s.reading_minutes,
        views: s.views,
        category: s.category,
        author: s.author,
        tags: s.tags,
        cover_url: s.cover_url,
        body: &article.body,
        updated_at: &article.updated_at,
        related,
    }
}
