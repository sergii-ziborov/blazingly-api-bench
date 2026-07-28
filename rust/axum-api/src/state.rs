//! Domain types, the seed loader and the in-memory store.
//!
//! `AppState` is an `Arc` so it can be handed to `Router::with_state` and
//! extracted with `State<AppState>` in every handler.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use serde::{Deserialize, Serialize};

pub type AppState = Arc<AppInner>;

pub const EDITOR_TOKEN: &str = "editor-token";
pub const ADMIN_TOKEN: &str = "admin-token";
pub const SCRAPER_KEY: &str = "scraper-key";

/// The spec fixes this at 100 requests per second per key. It is overridable so
/// the ingestion scenarios can be driven above the limit deliberately.
const DEFAULT_RATE_LIMIT: u32 = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub id: u32,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub id: u32,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct Author {
    pub id: u32,
    pub slug: String,
    pub name: String,
    pub bio: String,
}

#[derive(Debug, Deserialize)]
pub struct Article {
    pub id: u32,
    pub slug: String,
    pub title: String,
    pub excerpt: String,
    pub body: String,
    pub lang: String,
    /// `None` for a draft. Articles created through the editorial and
    /// ingestion surfaces start unpublished; `POST /admin/articles/{id}/publish`
    /// sets it, and publishing twice is a 409.
    #[serde(default)]
    pub published_at: Option<String>,
    pub updated_at: String,
    pub reading_minutes: u32,
    pub views: u64,
    pub category_id: u32,
    pub author_id: u32,
    pub tag_ids: Vec<u32>,
    pub cover_url: String,
    /// Lower-cased `title + excerpt`, built once at load so `?q=` and `/search`
    /// do not lower-case 1000 strings per request.
    #[serde(skip)]
    pub haystack: String,
    #[serde(skip)]
    pub deleted: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Company {
    pub id: u32,
    pub slug: String,
    pub name: String,
    pub industry: String,
    pub stage: String,
    pub founded_year: u32,
    pub employees: u32,
    pub total_funding_usd: u64,
    pub website: String,
    #[serde(skip)]
    pub haystack: String,
}

#[derive(Debug, Serialize)]
pub struct IngestRun {
    pub id: u32,
    pub source: String,
    pub started_at: String,
    pub finished_at: String,
    pub found: u32,
    pub ingested: u32,
    pub errors: u32,
}

#[derive(Deserialize)]
struct Seed {
    categories: Vec<Category>,
    tags: Vec<Tag>,
    authors: Vec<Author>,
    articles: Vec<Article>,
    companies: Vec<Company>,
}

/// Mutable half of the store.
///
/// `arena` is indexed by `id - 1` and never shrinks, so a deleted article
/// leaves a tombstone behind rather than shifting every other index. `order`
/// holds the live ids in listing order (`published_at` descending, then `id`
/// descending) and is the only thing iterated by the read paths.
pub struct Articles {
    pub arena: Vec<Article>,
    pub order: Vec<u32>,
    pub by_slug: HashMap<String, u32>,
}

impl Articles {
    fn key(article: &Article) -> (&str, u32) {
        (article.published_at.as_deref().unwrap_or(""), article.id)
    }

    pub fn get(&self, id: u32) -> Option<&Article> {
        let article = self.arena.get(id.checked_sub(1)? as usize)?;
        if article.deleted { None } else { Some(article) }
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Article> {
        let article = self.arena.get_mut(id.checked_sub(1)? as usize)?;
        if article.deleted { None } else { Some(article) }
    }

    pub fn find(&self, slug: &str) -> Option<&Article> {
        self.by_slug.get(slug).copied().and_then(|id| self.get(id))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Article> {
        self.order
            .iter()
            .filter_map(|id| self.arena.get((*id - 1) as usize))
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn next_id(&self) -> u32 {
        self.arena.len() as u32 + 1
    }

    pub fn contains_slug(&self, slug: &str) -> bool {
        self.by_slug.contains_key(slug)
    }

    fn placement(&self, id: u32) -> usize {
        let key = Self::key(&self.arena[(id - 1) as usize]);
        self.order
            .partition_point(|other| Self::key(&self.arena[(*other - 1) as usize]) > key)
    }

    fn place(&mut self, id: u32) {
        let index = self.placement(id);
        self.order.insert(index, id);
    }

    fn unplace(&mut self, id: u32) {
        if let Some(index) = self.order.iter().position(|other| *other == id) {
            self.order.remove(index);
        }
    }

    /// Re-sorts a single article after its `published_at` changed.
    pub fn reorder(&mut self, id: u32) {
        self.unplace(id);
        self.place(id);
    }

    pub fn insert(&mut self, article: Article) -> u32 {
        let id = article.id;
        self.by_slug.insert(article.slug.clone(), id);
        self.arena.push(article);
        self.place(id);
        id
    }

    pub fn rename(&mut self, id: u32, old_slug: &str, new_slug: &str) {
        self.by_slug.remove(old_slug);
        self.by_slug.insert(new_slug.to_owned(), id);
    }

    pub fn remove(&mut self, id: u32) -> bool {
        let Some(article) = self.get_mut(id) else {
            return false;
        };
        article.deleted = true;
        let slug = article.slug.clone();
        self.by_slug.remove(&slug);
        self.unplace(id);
        true
    }
}

pub struct AppInner {
    pub categories: Vec<Category>,
    pub tags: Vec<Tag>,
    pub authors: Vec<Author>,
    pub companies: Vec<Company>,
    pub category_by_slug: HashMap<String, u32>,
    pub tag_by_slug: HashMap<String, u32>,
    pub author_by_slug: HashMap<String, u32>,
    pub articles: RwLock<Articles>,
    pub runs: Mutex<Vec<IngestRun>>,
    pub limiter: Mutex<HashMap<String, (u64, u32)>>,
    pub rate_limit: u32,
    pub started: Instant,
}

impl AppInner {
    pub fn category(&self, id: u32) -> Option<&Category> {
        self.categories.get(id.checked_sub(1)? as usize)
    }

    pub fn tag(&self, id: u32) -> Option<&Tag> {
        self.tags.get(id.checked_sub(1)? as usize)
    }

    pub fn author(&self, id: u32) -> Option<&Author> {
        self.authors.get(id.checked_sub(1)? as usize)
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

fn seed_path() -> PathBuf {
    match std::env::var("BLAZINGLY_APIBENCH_SEED") {
        Ok(value) if !value.is_empty() => PathBuf::from(value),
        _ => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/seed.json"),
    }
}

pub fn load() -> Result<AppState, Box<dyn std::error::Error>> {
    let path = seed_path();
    let raw = std::fs::read(&path)
        .map_err(|error| format!("cannot read seed at {}: {error}", path.display()))?;
    let seed: Seed = serde_json::from_slice(&raw)?;

    let mut arena = seed.articles;
    arena.sort_unstable_by_key(|article| article.id);
    for (index, article) in arena.iter_mut().enumerate() {
        if article.id != index as u32 + 1 {
            return Err("seed article ids must be contiguous starting at 1".into());
        }
        article.haystack = format!("{} {}", article.title, article.excerpt).to_lowercase();
    }

    let mut order: Vec<u32> = arena.iter().map(|article| article.id).collect();
    order.sort_unstable_by(|left, right| {
        let left = &arena[(*left - 1) as usize];
        let right = &arena[(*right - 1) as usize];
        Articles::key(right).cmp(&Articles::key(left))
    });

    let by_slug = arena
        .iter()
        .map(|article| (article.slug.clone(), article.id))
        .collect();

    let mut companies = seed.companies;
    for company in &mut companies {
        company.haystack = format!("{} {}", company.name, company.industry).to_lowercase();
    }

    let category_by_slug = seed
        .categories
        .iter()
        .map(|item| (item.slug.clone(), item.id))
        .collect();
    let tag_by_slug = seed
        .tags
        .iter()
        .map(|item| (item.slug.clone(), item.id))
        .collect();
    let author_by_slug = seed
        .authors
        .iter()
        .map(|item| (item.slug.clone(), item.id))
        .collect();

    let rate_limit = std::env::var("APIBENCH_INGEST_RPS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(DEFAULT_RATE_LIMIT);

    Ok(Arc::new(AppInner {
        categories: seed.categories,
        tags: seed.tags,
        authors: seed.authors,
        companies,
        category_by_slug,
        tag_by_slug,
        author_by_slug,
        articles: RwLock::new(Articles {
            arena,
            order,
            by_slug,
        }),
        runs: Mutex::new(Vec::new()),
        limiter: Mutex::new(HashMap::new()),
        rate_limit,
        started: Instant::now(),
    }))
}
