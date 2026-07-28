//! Request payloads and their validation rules.
//!
//! Axum ships no validation, so the shape rules live on `validator` derives and
//! the rules that need the store (referential integrity, slug uniqueness) are
//! hand written next to them. Both paths funnel into `FieldError` so a caller
//! cannot tell which mechanism produced a given entry.

use std::borrow::Cow;

use serde::Deserialize;
use serde_json::Value;
use validator::{Validate, ValidationError, ValidationErrors, ValidationErrorsKind};

use crate::error::{ApiError, FieldError};

/// Contract limit for the cover upload.
pub const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;
/// Backstop for the tower-http body limit: the contract's 10 MiB applies to the
/// decoded file part, so the whole-request cap has to leave room for multipart
/// framing and is enforced again by hand while the part streams.
pub const UPLOAD_HARD_LIMIT: usize = MAX_UPLOAD_BYTES + 64 * 1024;

pub const LANGS: [&str; 3] = ["uk", "ru", "en"];
pub const STAGES: [&str; 4] = ["seed", "series_a", "series_b", "growth"];

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

fn error_with(code: &'static str, message: &'static str) -> ValidationError {
    let mut error = ValidationError::new(code);
    error.message = Some(Cow::Borrowed(message));
    error
}

fn validate_slug(value: &str) -> Result<(), ValidationError> {
    if slug_is_valid(value) {
        Ok(())
    } else {
        Err(error_with(
            "pattern",
            "must match ^[a-z0-9]+(-[a-z0-9]+)*$",
        ))
    }
}

fn validate_lang(value: &str) -> Result<(), ValidationError> {
    if lang_is_valid(value) {
        Ok(())
    } else {
        Err(error_with("one_of", "must be one of uk, ru, en"))
    }
}

fn validate_tag_ids(value: &[u32]) -> Result<(), ValidationError> {
    if tag_ids_are_unique(value) {
        Ok(())
    } else {
        Err(error_with("duplicate", "must not contain duplicate ids"))
    }
}

#[derive(Debug, Default, Deserialize, Validate)]
#[serde(default)]
pub struct CreateArticle {
    #[validate(length(min = 8, max = 200))]
    pub title: String,
    #[validate(length(min = 3, max = 200), custom(function = "validate_slug"))]
    pub slug: String,
    #[validate(length(min = 20, max = 500))]
    pub excerpt: String,
    #[validate(length(min = 50))]
    pub body: String,
    #[validate(custom(function = "validate_lang"))]
    pub lang: String,
    pub category_id: u32,
    pub author_id: u32,
    #[validate(length(max = 10), custom(function = "validate_tag_ids"))]
    pub tag_ids: Vec<u32>,
}

/// PATCH: every field optional, same rules when present.
///
/// `validator` runs `length` through `Option`, but its `custom` functions are
/// awkward to reuse across `T` and `Option<T>`, so the slug pattern, the `lang`
/// set and tag uniqueness are checked in `admin::update_article` with the same
/// helpers the derives above call.
#[derive(Debug, Default, Deserialize, Validate)]
#[serde(default)]
pub struct UpdateArticle {
    #[validate(length(min = 8, max = 200))]
    pub title: Option<String>,
    #[validate(length(min = 3, max = 200))]
    pub slug: Option<String>,
    #[validate(length(min = 20, max = 500))]
    pub excerpt: Option<String>,
    #[validate(length(min = 50))]
    pub body: Option<String>,
    pub lang: Option<String>,
    pub category_id: Option<u32>,
    pub author_id: Option<u32>,
    #[validate(length(max = 10))]
    pub tag_ids: Option<Vec<u32>>,
}

#[derive(Debug, Deserialize, Validate)]
pub struct PublishRequest {
    #[validate(length(min = 1))]
    pub published_at: String,
}

/// `#[validate(length(...))]` on `items` would be the obvious way to express
/// "1..=100 items", but `validator` records the offending value as a param and
/// so demands `Serialize` on `CreateArticle` — a trait a request payload has no
/// business implementing. The envelope size is checked in `ingest::bulk`.
#[derive(Debug, Deserialize, Validate)]
pub struct BulkRequest {
    pub items: Vec<CreateArticle>,
}

pub const BULK_MIN_ITEMS: usize = 1;
pub const BULK_MAX_ITEMS: usize = 100;

#[derive(Debug, Default, Deserialize, Validate)]
#[serde(default)]
pub struct CreateRun {
    #[validate(length(min = 1, max = 200))]
    pub source: String,
    #[validate(length(min = 1))]
    pub started_at: String,
    #[validate(length(min = 1))]
    pub finished_at: String,
    pub found: u32,
    pub ingested: u32,
    pub errors: u32,
}

#[derive(Debug, Deserialize, Validate)]
#[serde(default)]
pub struct ArticleQuery {
    #[validate(range(min = 1))]
    pub page: u32,
    #[validate(range(min = 1, max = 100))]
    pub limit: u32,
    pub category: Option<String>,
    pub tag: Option<String>,
    pub author: Option<String>,
    pub lang: Option<String>,
    pub q: Option<String>,
}

impl Default for ArticleQuery {
    fn default() -> Self {
        Self {
            page: 1,
            limit: 20,
            category: None,
            tag: None,
            author: None,
            lang: None,
            q: None,
        }
    }
}

#[derive(Debug, Deserialize, Validate)]
#[serde(default)]
pub struct CompanyQuery {
    #[validate(range(min = 1))]
    pub page: u32,
    #[validate(range(min = 1, max = 100))]
    pub limit: u32,
    pub industry: Option<String>,
    pub stage: Option<String>,
    pub min_funding: Option<u64>,
}

impl Default for CompanyQuery {
    fn default() -> Self {
        Self {
            page: 1,
            limit: 20,
            industry: None,
            stage: None,
            min_funding: None,
        }
    }
}

#[derive(Debug, Deserialize, Validate)]
pub struct SearchQuery {
    #[validate(length(min = 2, max = 100))]
    pub q: String,
}

/// Flattens `validator`'s nested error tree into the flat `{field, code,
/// message}` list the contract requires from the bulk endpoint.
pub fn flatten(errors: &ValidationErrors) -> Vec<FieldError> {
    let mut out = Vec::new();
    collect(errors, "", &mut out);
    out.sort_by(|left, right| left.field.cmp(&right.field));
    out
}

fn collect(errors: &ValidationErrors, prefix: &str, out: &mut Vec<FieldError>) {
    for (field, kind) in errors.errors() {
        let path = if prefix.is_empty() {
            field.to_string()
        } else {
            format!("{prefix}.{field}")
        };
        match kind {
            ValidationErrorsKind::Field(list) => {
                for error in list {
                    let (code, message) = describe(error);
                    out.push(FieldError::new(path.clone(), code, message));
                }
            }
            ValidationErrorsKind::Struct(inner) => collect(inner, &path, out),
            ValidationErrorsKind::List(entries) => {
                for (index, inner) in entries {
                    collect(inner, &format!("{path}[{index}]"), out);
                }
            }
        }
    }
}

/// `validator` reports one `length` code whether the minimum or the maximum was
/// violated, so the side is recovered from the params to produce the
/// `min_length` / `max_length` codes the contract's example shows.
fn describe(error: &ValidationError) -> (&str, String) {
    let min = error.params.get("min").and_then(Value::as_u64);
    let max = error.params.get("max").and_then(Value::as_u64);
    let mut unit = "characters";
    let measured = error.params.get("value").and_then(|value| match value {
        Value::String(text) => Some(text.chars().count() as u64),
        Value::Array(items) => {
            unit = "entries";
            Some(items.len() as u64)
        }
        _ => None,
    });

    let code = match error.code.as_ref() {
        "length" => match (measured, min) {
            (Some(measured), Some(min)) if measured < min => "min_length",
            (_, Some(_)) if max.is_none() => "min_length",
            _ => "max_length",
        },
        "range" => match (error.params.get("value").and_then(Value::as_u64), min) {
            (Some(measured), Some(min)) if measured < min => "min_value",
            (_, Some(_)) if max.is_none() => "min_value",
            _ => "max_value",
        },
        other => other,
    };

    if let Some(message) = &error.message {
        return (code, message.to_string());
    }
    let message = match code {
        "min_length" => format!("must be at least {} {unit}", min.unwrap_or_default()),
        "max_length" => format!("must be at most {} {unit}", max.unwrap_or_default()),
        "min_value" => format!("must be at least {}", min.unwrap_or_default()),
        "max_value" => format!("must be at most {}", max.unwrap_or_default()),
        other => format!("failed the {other} check"),
    };
    (code, message)
}

pub fn validated<T: Validate>(value: T) -> Result<T, ApiError> {
    match value.validate() {
        Ok(()) => Ok(value),
        Err(errors) => Err(ApiError::validation(flatten(&errors))),
    }
}
