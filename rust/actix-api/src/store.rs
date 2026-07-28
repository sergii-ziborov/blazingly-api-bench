//! In-memory corpus loaded from `data/seed.json`.
//!
//! `articles` is append-only so that the `by_slug` / `by_id` index positions
//! stay valid; `order` holds the contract's canonical listing order
//! (`published_at` descending, then `id` descending, unpublished drafts last)
//! and is the only thing that gets re-sorted on a write.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Instant;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Article {
    pub id: u64,
    pub slug: String,
    pub title: String,
    pub excerpt: String,
    pub body: String,
    pub lang: String,
    pub published_at: Option<String>,
    pub updated_at: String,
    pub reading_minutes: u32,
    pub views: u64,
    pub category_id: u32,
    pub author_id: u32,
    pub tag_ids: Vec<u32>,
    pub cover_url: String,
    /// Lower-cased `title + excerpt`, built once at load so `?q=` and `/search`
    /// do not lower-case a thousand strings on every request. Not part of the
    /// contract's wire shape.
    #[serde(skip)]
    pub haystack: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Category {
    pub id: u32,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Tag {
    pub id: u32,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Author {
    pub id: u32,
    pub slug: String,
    pub name: String,
    pub bio: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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
    /// Lower-cased `name + industry`, the two fields `/search` matches on.
    #[serde(skip)]
    pub haystack: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IngestRun {
    pub id: u64,
    pub source: String,
    pub started_at: String,
    pub finished_at: String,
    pub found: u64,
    pub ingested: u64,
    pub errors: u64,
}

#[derive(Deserialize)]
struct Seed {
    categories: Vec<Category>,
    tags: Vec<Tag>,
    authors: Vec<Author>,
    articles: Vec<Article>,
    companies: Vec<Company>,
}

pub struct Store {
    articles: Vec<Article>,
    order: Vec<usize>,
    by_slug: HashMap<String, usize>,
    by_id: HashMap<u64, usize>,

    pub categories: Vec<Category>,
    category_by_id: HashMap<u32, usize>,
    category_by_slug: HashMap<String, u32>,

    pub tags: Vec<Tag>,
    tag_by_id: HashMap<u32, usize>,
    tag_by_slug: HashMap<String, u32>,

    pub authors: Vec<Author>,
    author_by_id: HashMap<u32, usize>,
    author_by_slug: HashMap<String, u32>,

    pub companies: Vec<Company>,

    next_article_id: u64,
    pub runs: Vec<IngestRun>,
    next_run_id: u64,
}

/// The lower-cased text `?q=` and `/search` scan. Built at load and refreshed on
/// write, never per request.
pub fn haystack(first: &str, second: &str) -> String {
    format!("{first} {second}").to_lowercase()
}

/// Descending sort key. `Some(..)` outranks `None` so drafts land at the end.
fn order_key(article: &Article) -> (bool, &str, u64) {
    (article.published_at.is_some(), article.published_at.as_deref().unwrap_or(""), article.id)
}

impl Store {
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let raw = std::fs::read(path)?;
        let mut seed: Seed = serde_json::from_slice(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        for article in &mut seed.articles {
            article.haystack = haystack(&article.title, &article.excerpt);
        }
        for company in &mut seed.companies {
            company.haystack = haystack(&company.name, &company.industry);
        }

        let category_by_id = seed.categories.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
        let category_by_slug =
            seed.categories.iter().map(|c| (c.slug.clone(), c.id)).collect::<HashMap<_, _>>();
        let tag_by_id = seed.tags.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
        let tag_by_slug = seed.tags.iter().map(|t| (t.slug.clone(), t.id)).collect::<HashMap<_, _>>();
        let author_by_id = seed.authors.iter().enumerate().map(|(i, a)| (a.id, i)).collect();
        let author_by_slug =
            seed.authors.iter().map(|a| (a.slug.clone(), a.id)).collect::<HashMap<_, _>>();

        let by_slug = seed.articles.iter().enumerate().map(|(i, a)| (a.slug.clone(), i)).collect();
        let by_id: HashMap<u64, usize> =
            seed.articles.iter().enumerate().map(|(i, a)| (a.id, i)).collect();
        let next_article_id = seed.articles.iter().map(|a| a.id).max().unwrap_or(0) + 1;

        // The seed file is already in contract order, but sorting here means the
        // service never depends on that.
        let mut order: Vec<usize> = (0..seed.articles.len()).collect();
        order.sort_by(|&a, &b| order_key(&seed.articles[b]).cmp(&order_key(&seed.articles[a])));

        Ok(Store {
            articles: seed.articles,
            order,
            by_slug,
            by_id,
            categories: seed.categories,
            category_by_id,
            category_by_slug,
            tags: seed.tags,
            tag_by_id,
            tag_by_slug,
            authors: seed.authors,
            author_by_id,
            author_by_slug,
            companies: seed.companies,
            next_article_id,
            runs: Vec::new(),
            next_run_id: 1,
        })
    }

    /// Articles in contract order.
    pub fn listing(&self) -> impl Iterator<Item = &Article> {
        self.order.iter().map(|&i| &self.articles[i])
    }

    pub fn article_count(&self) -> usize {
        self.order.len()
    }

    pub fn newest(&self) -> Option<&Article> {
        self.order.first().map(|&i| &self.articles[i])
    }

    pub fn article_by_slug(&self, slug: &str) -> Option<&Article> {
        self.by_slug.get(slug).map(|&i| &self.articles[i])
    }

    pub fn article_by_id(&self, id: u64) -> Option<&Article> {
        self.by_id.get(&id).map(|&i| &self.articles[i])
    }

    pub fn category(&self, id: u32) -> Option<&Category> {
        self.category_by_id.get(&id).map(|&i| &self.categories[i])
    }

    pub fn category_id_of(&self, slug: &str) -> Option<u32> {
        self.category_by_slug.get(slug).copied()
    }

    pub fn tag(&self, id: u32) -> Option<&Tag> {
        self.tag_by_id.get(&id).map(|&i| &self.tags[i])
    }

    pub fn tag_id_of(&self, slug: &str) -> Option<u32> {
        self.tag_by_slug.get(slug).copied()
    }

    pub fn author(&self, id: u32) -> Option<&Author> {
        self.author_by_id.get(&id).map(|&i| &self.authors[i])
    }

    pub fn author_by_slug(&self, slug: &str) -> Option<&Author> {
        self.author_by_slug.get(slug).and_then(|&id| self.author(id))
    }

    pub fn next_article_id(&mut self) -> u64 {
        let id = self.next_article_id;
        self.next_article_id += 1;
        id
    }

    pub fn insert_article(&mut self, article: Article) -> u64 {
        let id = article.id;
        let index = self.articles.len();
        self.by_slug.insert(article.slug.clone(), index);
        self.by_id.insert(id, index);
        self.articles.push(article);
        let key = order_key(&self.articles[index]);
        let at = self.order.partition_point(|&i| order_key(&self.articles[i]) > key);
        self.order.insert(at, index);
        id
    }

    pub fn remove_article(&mut self, id: u64) -> bool {
        let Some(index) = self.by_id.remove(&id) else { return false };
        self.by_slug.remove(&self.articles[index].slug);
        self.order.retain(|&i| i != index);
        true
    }

    /// Applies `edit` to the article and re-files it in `order`; also keeps the
    /// slug index in sync when the slug changed.
    pub fn update_article(&mut self, id: u64, edit: impl FnOnce(&mut Article)) -> Option<&Article> {
        let index = *self.by_id.get(&id)?;
        let old_slug = self.articles[index].slug.clone();
        edit(&mut self.articles[index]);
        if self.articles[index].slug != old_slug {
            self.by_slug.remove(&old_slug);
            self.by_slug.insert(self.articles[index].slug.clone(), index);
        }
        self.order.retain(|&i| i != index);
        let key = order_key(&self.articles[index]);
        let at = self.order.partition_point(|&i| order_key(&self.articles[i]) > key);
        self.order.insert(at, index);
        Some(&self.articles[index])
    }

    pub fn next_run_id(&mut self) -> u64 {
        let id = self.next_run_id;
        self.next_run_id += 1;
        id
    }
}

/// Fixed one-second window per API key, shared across all `HttpServer` workers.
pub struct RateLimiter {
    windows: Mutex<HashMap<String, (Instant, u32)>>,
    per_second: u32,
}

impl RateLimiter {
    pub fn new(per_second: u32) -> Self {
        Self { windows: Mutex::new(HashMap::new()), per_second }
    }

    /// `true` when the request is allowed.
    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        let entry = windows.entry(key.to_owned()).or_insert((now, 0));
        if now.duration_since(entry.0).as_secs() >= 1 {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= self.per_second
    }
}

/// Everything reachable from a handler via `web::Data<AppState>`.
pub struct AppState {
    store: RwLock<Store>,
    pub limiter: RateLimiter,
    started: Instant,
}

impl AppState {
    pub fn new(store: Store, requests_per_second: u32) -> Self {
        Self {
            store: RwLock::new(store),
            limiter: RateLimiter::new(requests_per_second),
            started: Instant::now(),
        }
    }

    /// Lock poisoning is not a meaningful state for this service: no handler
    /// leaves the store half-written, so recover rather than propagate.
    pub fn read(&self) -> RwLockReadGuard<'_, Store> {
        self.store.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, Store> {
        self.store.write().unwrap_or_else(|e| e.into_inner())
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}
