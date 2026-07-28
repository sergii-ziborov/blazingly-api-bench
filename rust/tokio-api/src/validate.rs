//! Request payloads and their validation rules.
//!
//! The framework implementations reach for `validator` and then spend a page
//! translating its nested error tree back into the flat `{field, code,
//! message}` list the contract wants. With no framework in the way there is
//! nothing to integrate with, so the rules are written directly and emit that
//! shape in the first place. The codes and messages are kept identical to the
//! other implementations — the point of the comparison is the framework layer,
//! not the wording of the errors.

use serde::Deserialize;

use crate::error::{ApiError, FieldError};
use crate::store::{AppInner, Articles};

/// Contract limit for the cover upload.
pub const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;
/// Backstop for the whole request: the contract's 10 MiB applies to the decoded
/// file part, so the request cap has to leave room for multipart framing. The
/// part itself is counted again as it streams.
pub const UPLOAD_HARD_LIMIT: usize = MAX_UPLOAD_BYTES + 64 * 1024;
/// Cap on a JSON request body. 100 bulk items are ~55 KiB; this is the same
/// order of magnitude as the frameworks' own defaults.
pub const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;

pub const LANGS: [&str; 3] = ["uk", "ru", "en"];
pub const STAGES: [&str; 4] = ["seed", "series_a", "series_b", "growth"];

pub const BULK_MIN_ITEMS: usize = 1;
pub const BULK_MAX_ITEMS: usize = 100;
pub const MAX_TAG_IDS: usize = 10;

pub fn lang_is_valid(lang: &str) -> bool {
    LANGS.contains(&lang)
}

pub fn stage_is_valid(stage: &str) -> bool {
    STAGES.contains(&stage)
}

/// `^[a-z0-9]+(-[a-z0-9]+)*$` without pulling in `regex` for a single pattern.
pub fn slug_is_valid(slug: &str) -> bool {
    !slug.is_empty()
        && slug.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

pub fn tag_ids_are_unique(ids: &[u32]) -> bool {
    ids.iter()
        .enumerate()
        .all(|(index, id)| !ids[index + 1..].contains(id))
}

fn char_len(value: &str) -> usize {
    value.chars().count()
}

fn check_length(
    errors: &mut Vec<FieldError>,
    field: &str,
    value: &str,
    min: usize,
    max: Option<usize>,
) {
    let length = char_len(value);
    if length < min {
        errors.push(FieldError::new(
            field,
            "min_length",
            format!("must be at least {min} characters"),
        ));
        return;
    }
    if let Some(max) = max
        && length > max
    {
        errors.push(FieldError::new(
            field,
            "max_length",
            format!("must be at most {max} characters"),
        ));
    }
}

fn check_lang(errors: &mut Vec<FieldError>, lang: &str) {
    if !lang_is_valid(lang) {
        errors.push(FieldError::new(
            "lang",
            "one_of",
            "must be one of uk, ru, en",
        ));
    }
}

fn check_slug_pattern(errors: &mut Vec<FieldError>, slug: &str) {
    if !slug_is_valid(slug) {
        errors.push(FieldError::new(
            "slug",
            "pattern",
            "must match ^[a-z0-9]+(-[a-z0-9]+)*$",
        ));
    }
}

fn check_tag_ids(errors: &mut Vec<FieldError>, tag_ids: &[u32]) {
    if tag_ids.len() > MAX_TAG_IDS {
        errors.push(FieldError::new(
            "tag_ids",
            "max_length",
            format!("must be at most {MAX_TAG_IDS} entries"),
        ));
    }
    if !tag_ids_are_unique(tag_ids) {
        errors.push(FieldError::new(
            "tag_ids",
            "duplicate",
            "must not contain duplicate ids",
        ));
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CreateArticle {
    pub title: String,
    pub slug: String,
    pub excerpt: String,
    pub body: String,
    pub lang: String,
    pub category_id: u32,
    pub author_id: u32,
    pub tag_ids: Vec<u32>,
}

/// Shape rules only. Emitted in field order so the response is deterministic,
/// which is what the other implementations get by sorting `validator` output.
pub fn shape_errors(input: &CreateArticle) -> Vec<FieldError> {
    let mut errors = Vec::new();
    check_length(&mut errors, "body", &input.body, 50, None);
    check_length(&mut errors, "excerpt", &input.excerpt, 20, Some(500));
    check_lang(&mut errors, &input.lang);
    check_length(&mut errors, "slug", &input.slug, 3, Some(200));
    check_slug_pattern(&mut errors, &input.slug);
    check_tag_ids(&mut errors, &input.tag_ids);
    check_length(&mut errors, "title", &input.title, 8, Some(200));
    errors
}

/// PATCH: every field optional, same rules when present.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct UpdateArticle {
    pub title: Option<String>,
    pub slug: Option<String>,
    pub excerpt: Option<String>,
    pub body: Option<String>,
    pub lang: Option<String>,
    pub category_id: Option<u32>,
    pub author_id: Option<u32>,
    pub tag_ids: Option<Vec<u32>>,
}

pub fn update_shape_errors(input: &UpdateArticle) -> Vec<FieldError> {
    let mut errors = Vec::new();
    if let Some(body) = &input.body {
        check_length(&mut errors, "body", body, 50, None);
    }
    if let Some(excerpt) = &input.excerpt {
        check_length(&mut errors, "excerpt", excerpt, 20, Some(500));
    }
    if let Some(lang) = &input.lang {
        check_lang(&mut errors, lang);
    }
    if let Some(slug) = &input.slug {
        check_length(&mut errors, "slug", slug, 3, Some(200));
        check_slug_pattern(&mut errors, slug);
    }
    if let Some(tag_ids) = &input.tag_ids {
        check_tag_ids(&mut errors, tag_ids);
    }
    if let Some(title) = &input.title {
        check_length(&mut errors, "title", title, 8, Some(200));
    }
    errors
}

#[derive(Debug, Deserialize)]
pub struct PublishRequest {
    pub published_at: String,
}

#[derive(Debug, Deserialize)]
pub struct BulkRequest {
    pub items: Vec<CreateArticle>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CreateRun {
    pub source: String,
    pub started_at: String,
    pub finished_at: String,
    pub found: u32,
    pub ingested: u32,
    pub errors: u32,
}

pub fn run_shape_errors(input: &CreateRun) -> Vec<FieldError> {
    let mut errors = Vec::new();
    check_length(&mut errors, "finished_at", &input.finished_at, 1, None);
    check_length(&mut errors, "source", &input.source, 1, Some(200));
    check_length(&mut errors, "started_at", &input.started_at, 1, None);
    errors
}

/// The rules that need the store: referential integrity plus slug uniqueness.
pub fn reference_errors(
    state: &AppInner,
    articles: &Articles,
    category_id: Option<u32>,
    author_id: Option<u32>,
    tag_ids: Option<&[u32]>,
    slug: Option<(&str, Option<u32>)>,
) -> Vec<FieldError> {
    let mut errors = Vec::new();
    if let Some(id) = category_id
        && state.category(id).is_none()
    {
        errors.push(FieldError::new(
            "category_id",
            "unknown_reference",
            "category does not exist",
        ));
    }
    if let Some(id) = author_id
        && state.author(id).is_none()
    {
        errors.push(FieldError::new(
            "author_id",
            "unknown_reference",
            "author does not exist",
        ));
    }
    if let Some(ids) = tag_ids
        && ids.iter().any(|id| state.tag(*id).is_none())
    {
        errors.push(FieldError::new(
            "tag_ids",
            "unknown_reference",
            "one or more tags do not exist",
        ));
    }
    if let Some((slug, owner)) = slug
        && let Some(existing) = articles.by_slug.get(slug)
        && Some(*existing) != owner
    {
        errors.push(FieldError::new("slug", "duplicate", "slug already exists"));
    }
    errors
}

pub fn fail_if(errors: Vec<FieldError>) -> Result<(), ApiError> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ApiError::validation(errors))
    }
}
