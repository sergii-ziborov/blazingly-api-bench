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

use crate::models::{
    ArticleSummary, AuthorView, Company, CreateArticle, IngestRun, Ref, TaxonomyView,
};

/// One article as `data/seed.json` and the write endpoints describe it: the
/// eight client-supplied fields plus the six the server owns.
#[derive(Clone, Debug, Deserialize)]
pub struct RawArticle {
    pub id: u32,
    #[serde(flatten)]
    pub input: CreateArticle,
    /// `None` for an article created through the editorial API and not yet
    /// published; publishing is what fills it in.
    #[serde(default)]
    pub published_at: Option<String>,
    pub updated_at: String,
    pub reading_minutes: u32,
    pub views: u64,
    pub cover_url: String,
}

/// One stored article: the exact public projection, plus what only the detail
/// view and the substring filter need.
#[derive(Clone, Debug)]
pub struct Article {
    pub summary: ArticleSummary,
    pub body: String,
    pub updated_at: String,
    /// Lower-cased `title + excerpt`, built once when the article is stored so
    /// `?q=` and `/search` do not lower-case a thousand strings per request.
    pub haystack: String,
}

impl Article {
    /// The contract's listing order: `published_at` descending, then `id`
    /// descending, so an unpublished draft sorts last.
    fn order_key(&self) -> (&str, u32) {
        (
            self.summary.published_at.as_deref().unwrap_or(""),
            self.summary.id,
        )
    }
}

pub fn haystack(left: &str, right: &str) -> String {
    format!("{left} {right}").to_lowercase()
}

#[derive(Debug, Deserialize)]
struct Seed {
    categories: Vec<TaxonomyView>,
    tags: Vec<TaxonomyView>,
    authors: Vec<AuthorView>,
    articles: Vec<RawArticle>,
    companies: Vec<Company>,
}

/// Everything the contract never mutates.
///
/// Category, tag and author ids are contiguous from 1, so lookups index rather
/// than hash; only the slug direction needs a map.
pub struct Corpus {
    pub categories: Vec<TaxonomyView>,
    pub tags: Vec<TaxonomyView>,
    pub authors: Vec<AuthorView>,
    pub companies: Vec<Company>,
    /// Lower-cased `name + industry`, in `companies` order.
    pub company_haystacks: Vec<String>,
    pub category_by_slug: HashMap<String, u32>,
    pub tag_by_slug: HashMap<String, u32>,
    pub author_by_slug: HashMap<String, u32>,
}

impl Corpus {
    fn new(
        categories: Vec<TaxonomyView>,
        tags: Vec<TaxonomyView>,
        authors: Vec<AuthorView>,
        companies: Vec<Company>,
    ) -> Self {
        Self {
            company_haystacks: companies
                .iter()
                .map(|company| haystack(&company.name, &company.industry))
                .collect(),
            category_by_slug: slug_index(categories.iter().map(|view| &view.taxon)),
            tag_by_slug: slug_index(tags.iter().map(|view| &view.taxon)),
            author_by_slug: slug_index(authors.iter().map(|view| &view.author)),
            categories,
            tags,
            authors,
            companies,
        }
    }

    pub fn category(&self, id: u32) -> Option<&TaxonomyView> {
        self.categories.get(id.checked_sub(1)? as usize)
    }

    pub fn tag(&self, id: u32) -> Option<&TaxonomyView> {
        self.tags.get(id.checked_sub(1)? as usize)
    }

    pub fn author(&self, id: u32) -> Option<&AuthorView> {
        self.authors.get(id.checked_sub(1)? as usize)
    }

    /// Resolves a raw article into its stored form once, so every later read is
    /// a clone of a finished projection.
    pub fn assemble(&self, raw: RawArticle) -> Article {
        let input = raw.input;
        Article {
            haystack: haystack(&input.title, &input.excerpt),
            summary: ArticleSummary {
                id: raw.id,
                slug: input.slug,
                title: input.title,
                excerpt: input.excerpt,
                lang: input.lang,
                published_at: raw.published_at,
                reading_minutes: raw.reading_minutes,
                views: raw.views,
                category: reference(self.category(input.category_id).map(|it| &it.taxon)),
                author: reference(self.author(input.author_id).map(|it| &it.author)),
                tags: input
                    .tag_ids
                    .iter()
                    .filter_map(|id| self.tag(*id))
                    .map(|tag| tag.taxon.clone())
                    .collect(),
                cover_url: raw.cover_url,
            },
            body: input.body,
            updated_at: raw.updated_at,
        }
    }
}

fn slug_index<'a>(refs: impl Iterator<Item = &'a Ref>) -> HashMap<String, u32> {
    refs.map(|item| (item.slug.clone(), item.id)).collect()
}

fn reference(found: Option<&Ref>) -> Ref {
    found.cloned().unwrap_or_default()
}

/// Articles in the contract's listing order, shared as `Arc` so the id and slug
/// indexes cost a refcount rather than a second copy.
pub struct Articles {
    /// The contract's listing order; every read path iterates this.
    pub ordered: Vec<Arc<Article>>,
    pub by_slug: HashMap<String, Arc<Article>>,
    pub by_id: HashMap<u32, Arc<Article>>,
    next_id: u32,
}

impl Articles {
    /// The seed is already in listing order, so it is adopted as-is.
    fn new(ordered: Vec<Arc<Article>>) -> Self {
        let mut by_slug = HashMap::with_capacity(ordered.len());
        let mut by_id = HashMap::with_capacity(ordered.len());
        let mut next_id = 1;
        for article in &ordered {
            by_slug.insert(article.summary.slug.clone(), Arc::clone(article));
            by_id.insert(article.summary.id, Arc::clone(article));
            next_id = next_id.max(article.summary.id + 1);
        }
        Self {
            ordered,
            by_slug,
            by_id,
            next_id,
        }
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
        let article = Arc::new(article);
        self.by_slug
            .insert(article.summary.slug.clone(), Arc::clone(&article));
        self.by_id.insert(article.summary.id, Arc::clone(&article));
        let at = self
            .ordered
            .partition_point(|other| other.order_key() > article.order_key());
        self.ordered.insert(at, Arc::clone(&article));
        article
    }

    /// Replaces an article, re-sorting it if publishing changed its position.
    pub fn replace(&mut self, previous_slug: &str, article: Article) -> Arc<Article> {
        let id = article.summary.id;
        self.by_slug.remove(previous_slug);
        self.ordered.retain(|other| other.summary.id != id);
        self.insert(article)
    }

    pub fn remove(&mut self, id: u32) -> bool {
        let Some(article) = self.by_id.remove(&id) else {
            return false;
        };
        self.by_slug.remove(&article.summary.slug);
        self.ordered.retain(|other| other.summary.id != id);
        true
    }
}

#[derive(Default)]
pub struct Runs {
    stored: Vec<IngestRun>,
    next_id: u64,
}

impl Runs {
    pub fn record(&mut self, mut run: IngestRun) -> IngestRun {
        self.next_id += 1;
        run.id = self.next_id;
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
            std::io::Error::other(format!(
                "cannot read the seed at {}: {error}",
                path.display()
            ))
        })?;
        let seed: Seed = serde_json::from_slice(&bytes)
            .map_err(|error| std::io::Error::other(format!("seed corpus is not valid: {error}")))?;
        let corpus = Corpus::new(seed.categories, seed.tags, seed.authors, seed.companies);
        let ordered = seed
            .articles
            .into_iter()
            .map(|raw| Arc::new(corpus.assemble(raw)))
            .collect();
        Ok(Self(Arc::new(Inner {
            started: Instant::now(),
            corpus,
            articles: RwLock::new(Articles::new(ordered)),
            runs: RwLock::new(Runs::default()),
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
