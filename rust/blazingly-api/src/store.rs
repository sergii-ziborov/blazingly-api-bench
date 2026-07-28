//! The in-memory corpus loaded from `data/seed.json`.
//!
//! `MulticoreServer` builds one compiled application per worker and keeps the
//! dependency graph thread-local, so anything a write endpoint mutates has to
//! live behind an `Arc` that every worker shares. Taxonomies never change and
//! stay outside the lock; articles and ingestion runs do not.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Instant;

#[derive(Clone, Debug, Deserialize)]
pub struct Taxon {
    pub id: u32,
    pub slug: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Author {
    pub id: u32,
    pub slug: String,
    pub name: String,
    pub bio: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Company {
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

#[derive(Clone, Debug, Deserialize)]
pub struct Article {
    pub id: u32,
    pub slug: String,
    pub title: String,
    pub excerpt: String,
    pub body: String,
    pub lang: String,
    /// `None` for an article created through the editorial API and not yet
    /// published; publishing is what fills it in.
    pub published_at: Option<String>,
    pub updated_at: String,
    pub reading_minutes: u32,
    pub views: u64,
    pub category_id: u32,
    pub author_id: u32,
    pub tag_ids: Vec<u32>,
    pub cover_url: String,
}

#[derive(Debug, Deserialize)]
struct Seed {
    categories: Vec<Taxon>,
    tags: Vec<Taxon>,
    authors: Vec<Author>,
    articles: Vec<Article>,
    companies: Vec<Company>,
}

/// Everything the contract never mutates.
pub struct Corpus {
    pub categories: Vec<Taxon>,
    pub tags: Vec<Taxon>,
    pub authors: Vec<Author>,
    pub companies: Vec<Company>,
    category_by_id: HashMap<u32, usize>,
    category_by_slug: HashMap<String, u32>,
    tag_by_id: HashMap<u32, usize>,
    tag_by_slug: HashMap<String, u32>,
    author_by_id: HashMap<u32, usize>,
    author_by_slug: HashMap<String, u32>,
}

impl Corpus {
    fn new(
        categories: Vec<Taxon>,
        tags: Vec<Taxon>,
        authors: Vec<Author>,
        companies: Vec<Company>,
    ) -> Self {
        let category_by_id = index_by_id(&categories, |taxon| taxon.id);
        let category_by_slug = index_by_slug(&categories, |taxon| (&taxon.slug, taxon.id));
        let tag_by_id = index_by_id(&tags, |taxon| taxon.id);
        let tag_by_slug = index_by_slug(&tags, |taxon| (&taxon.slug, taxon.id));
        let author_by_id = index_by_id(&authors, |author| author.id);
        let author_by_slug = index_by_slug(&authors, |author| (&author.slug, author.id));
        Self {
            categories,
            tags,
            authors,
            companies,
            category_by_id,
            category_by_slug,
            tag_by_id,
            tag_by_slug,
            author_by_id,
            author_by_slug,
        }
    }

    pub fn category(&self, id: u32) -> Option<&Taxon> {
        self.category_by_id
            .get(&id)
            .and_then(|index| self.categories.get(*index))
    }

    pub fn tag(&self, id: u32) -> Option<&Taxon> {
        self.tag_by_id
            .get(&id)
            .and_then(|index| self.tags.get(*index))
    }

    pub fn author(&self, id: u32) -> Option<&Author> {
        self.author_by_id
            .get(&id)
            .and_then(|index| self.authors.get(*index))
    }

    pub fn category_id(&self, slug: &str) -> Option<u32> {
        self.category_by_slug.get(slug).copied()
    }

    pub fn tag_id(&self, slug: &str) -> Option<u32> {
        self.tag_by_slug.get(slug).copied()
    }

    pub fn author_id(&self, slug: &str) -> Option<u32> {
        self.author_by_slug.get(slug).copied()
    }
}

/// Articles in ascending display order: the *last* entry is the newest, so a
/// listing iterates in reverse and an insert is a push rather than a shift of
/// a thousand elements per bulk item.
pub struct Articles {
    ordered: Vec<Arc<Article>>,
    by_slug: HashMap<String, usize>,
    by_id: HashMap<u32, usize>,
    next_id: u32,
}

impl Articles {
    fn new(mut seeded: Vec<Article>) -> Self {
        let next_id = seeded.iter().map(|article| article.id).max().unwrap_or(0) + 1;
        seeded.reverse();
        let ordered: Vec<Arc<Article>> = seeded.into_iter().map(Arc::new).collect();
        let mut by_slug = HashMap::with_capacity(ordered.len());
        let mut by_id = HashMap::with_capacity(ordered.len());
        for (index, article) in ordered.iter().enumerate() {
            by_slug.insert(article.slug.clone(), index);
            by_id.insert(article.id, index);
        }
        Self {
            ordered,
            by_slug,
            by_id,
            next_id,
        }
    }

    /// Ascending order; callers that want the contract's default ordering
    /// iterate this in reverse.
    pub fn ascending(&self) -> &[Arc<Article>] {
        &self.ordered
    }

    pub fn len(&self) -> usize {
        self.ordered.len()
    }

    pub fn newest(&self) -> Option<&Arc<Article>> {
        self.ordered.last()
    }

    pub fn by_slug(&self, slug: &str) -> Option<&Arc<Article>> {
        self.by_slug
            .get(slug)
            .and_then(|index| self.ordered.get(*index))
    }

    pub fn by_id(&self, id: u32) -> Option<&Arc<Article>> {
        self.by_id
            .get(&id)
            .and_then(|index| self.ordered.get(*index))
    }

    pub fn contains_slug(&self, slug: &str) -> bool {
        self.by_slug.contains_key(slug)
    }

    pub fn take_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn insert(&mut self, article: Article) -> Arc<Article> {
        let index = self.ordered.len();
        self.by_slug.insert(article.slug.clone(), index);
        self.by_id.insert(article.id, index);
        let article = Arc::new(article);
        self.ordered.push(Arc::clone(&article));
        article
    }

    /// Replaces an article in place, keeping its position in the listing.
    pub fn replace(&mut self, previous_slug: &str, article: Article) -> Arc<Article> {
        let index = self.by_id.get(&article.id).copied().unwrap_or_default();
        if previous_slug != article.slug {
            self.by_slug.remove(previous_slug);
            self.by_slug.insert(article.slug.clone(), index);
        }
        let article = Arc::new(article);
        self.ordered[index] = Arc::clone(&article);
        article
    }

    pub fn remove(&mut self, id: u32) -> bool {
        let Some(index) = self.by_id.remove(&id) else {
            return false;
        };
        let removed = self.ordered.remove(index);
        self.by_slug.remove(&removed.slug);
        for position in self.by_slug.values_mut() {
            if *position > index {
                *position -= 1;
            }
        }
        for position in self.by_id.values_mut() {
            if *position > index {
                *position -= 1;
            }
        }
        true
    }
}

#[derive(Clone, Debug)]
pub struct IngestRun {
    pub id: u64,
    pub source: String,
    pub started_at: String,
    pub finished_at: String,
    pub found: u32,
    pub ingested: u32,
    pub errors: u32,
}

pub struct Runs {
    stored: Vec<IngestRun>,
    next_id: u64,
}

impl Runs {
    pub fn record(&mut self, mut run: IngestRun) -> IngestRun {
        run.id = self.next_id;
        self.next_id += 1;
        self.stored.push(run.clone());
        run
    }
}

pub struct Inner {
    pub started: Instant,
    pub corpus: Corpus,
    pub articles: RwLock<Articles>,
    pub runs: RwLock<Runs>,
}

/// The handle every operation receives as a typed dependency.
#[derive(Clone)]
pub struct AppState(Arc<Inner>);

impl std::ops::Deref for AppState {
    type Target = Inner;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AppState {
    pub fn load() -> std::io::Result<Self> {
        let path = seed_path();
        let bytes = std::fs::read(&path).map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!(
                    "could not read the seed corpus at {}: {error}",
                    path.display()
                ),
            )
        })?;
        let seed: Seed = serde_json::from_slice(&bytes)
            .map_err(|error| std::io::Error::other(format!("seed corpus is not valid: {error}")))?;
        Ok(Self(Arc::new(Inner {
            started: Instant::now(),
            corpus: Corpus::new(seed.categories, seed.tags, seed.authors, seed.companies),
            articles: RwLock::new(Articles::new(seed.articles)),
            runs: RwLock::new(Runs {
                stored: Vec::new(),
                next_id: 1,
            }),
        })))
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

/// `BLAZINGLY_APIBENCH_SEED` wins; otherwise the corpus is resolved relative to
/// this crate so the binary works from any working directory.
fn seed_path() -> PathBuf {
    if let Some(configured) = std::env::var_os("BLAZINGLY_APIBENCH_SEED") {
        return PathBuf::from(configured);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/seed.json")
}

fn index_by_id<T>(items: &[T], id: impl Fn(&T) -> u32) -> HashMap<u32, usize> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| (id(item), index))
        .collect()
}

fn index_by_slug<T>(items: &[T], key: impl Fn(&T) -> (&String, u32)) -> HashMap<String, u32> {
    items
        .iter()
        .map(|item| {
            let (slug, id) = key(item);
            (slug.clone(), id)
        })
        .collect()
}
