//! Borrowed response views.
//!
//! Every read path holds the store's `RwLockReadGuard` while it serialises, so
//! the views borrow titles, bodies and tags straight out of the arena instead
//! of cloning them per request.

use serde::Serialize;

use crate::store::{AppInner, Article, Articles, Category, Company, Tag};

#[derive(Serialize)]
pub struct AuthorRef<'a> {
    pub id: u32,
    pub slug: &'a str,
    pub name: &'a str,
}

#[derive(Serialize)]
pub struct Summary<'a> {
    pub id: u32,
    pub slug: &'a str,
    pub title: &'a str,
    pub excerpt: &'a str,
    pub lang: &'a str,
    pub published_at: Option<&'a str>,
    pub reading_minutes: u32,
    pub views: u64,
    pub category: Option<&'a Category>,
    pub author: Option<AuthorRef<'a>>,
    pub tags: Vec<&'a Tag>,
    pub cover_url: &'a str,
}

#[derive(Serialize)]
pub struct Detail<'a> {
    pub id: u32,
    pub slug: &'a str,
    pub title: &'a str,
    pub excerpt: &'a str,
    pub lang: &'a str,
    pub published_at: Option<&'a str>,
    pub reading_minutes: u32,
    pub views: u64,
    pub category: Option<&'a Category>,
    pub author: Option<AuthorRef<'a>>,
    pub tags: Vec<&'a Tag>,
    pub cover_url: &'a str,
    pub body: &'a str,
    pub updated_at: &'a str,
    pub related: Vec<Summary<'a>>,
}

#[derive(Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: u32,
    pub limit: u32,
    pub total: usize,
    pub pages: usize,
}

#[derive(Serialize)]
pub struct Facet<'a> {
    pub id: u32,
    pub slug: &'a str,
    pub name: &'a str,
    pub article_count: usize,
}

#[derive(Serialize)]
pub struct AuthorProfile<'a> {
    pub id: u32,
    pub slug: &'a str,
    pub name: &'a str,
    pub bio: &'a str,
    pub article_count: usize,
}

#[derive(Serialize)]
pub struct SearchResults<'a> {
    pub query: &'a str,
    pub articles: Vec<Summary<'a>>,
    pub companies: Vec<&'a Company>,
}

pub fn summary<'a>(state: &'a AppInner, article: &'a Article) -> Summary<'a> {
    Summary {
        id: article.id,
        slug: &article.slug,
        title: &article.title,
        excerpt: &article.excerpt,
        lang: &article.lang,
        published_at: article.published_at.as_deref(),
        reading_minutes: article.reading_minutes,
        views: article.views,
        category: state.category(article.category_id),
        author: state.author(article.author_id).map(|author| AuthorRef {
            id: author.id,
            slug: &author.slug,
            name: &author.name,
        }),
        tags: article
            .tag_ids
            .iter()
            .filter_map(|id| state.tag(*id))
            .collect(),
        cover_url: &article.cover_url,
    }
}

pub fn detail<'a>(state: &'a AppInner, articles: &'a Articles, article: &'a Article) -> Detail<'a> {
    let related = articles
        .iter()
        .filter(|other| other.category_id == article.category_id && other.id != article.id)
        .take(3)
        .map(|other| summary(state, other))
        .collect();

    Detail {
        id: article.id,
        slug: &article.slug,
        title: &article.title,
        excerpt: &article.excerpt,
        lang: &article.lang,
        published_at: article.published_at.as_deref(),
        reading_minutes: article.reading_minutes,
        views: article.views,
        category: state.category(article.category_id),
        author: state.author(article.author_id).map(|author| AuthorRef {
            id: author.id,
            slug: &author.slug,
            name: &author.name,
        }),
        tags: article
            .tag_ids
            .iter()
            .filter_map(|id| state.tag(*id))
            .collect(),
        cover_url: &article.cover_url,
        body: &article.body,
        updated_at: &article.updated_at,
        related,
    }
}
