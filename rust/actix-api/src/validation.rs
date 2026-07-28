//! Request bodies and their validation.
//!
//! Actix ships no validation of its own, so this uses `validator` — the crate
//! the Rust web ecosystem defaults to. `validator` covers the syntactic rules
//! (lengths, formats) declaratively; anything that needs to look at the store
//! (does this category exist? is this slug taken?) is a hand-written pass,
//! because `validator`'s context support cannot express "three separate errors
//! on one field" and cannot see `&Store` without threading a `'v_a` lifetime
//! through the derive.

use serde::Deserialize;
use serde_json::Value;
use validator::{Validate, ValidationError, ValidationErrors};

use crate::error::FieldError;
use crate::store::Store;

pub const LANGS: [&str; 3] = ["uk", "ru", "en"];
pub const STAGES: [&str; 4] = ["seed", "series_a", "series_b", "growth"];

#[derive(Debug, Deserialize, Validate)]
pub struct CreateArticle {
    #[validate(length(min = 8, max = 200))]
    pub title: String,
    #[validate(length(min = 3, max = 200), custom(function = "slug_format"))]
    pub slug: String,
    #[validate(length(min = 20, max = 500))]
    pub excerpt: String,
    #[validate(length(min = 50))]
    pub body: String,
    #[validate(custom(function = "lang_code"))]
    pub lang: String,
    pub category_id: u32,
    pub author_id: u32,
    #[serde(default)]
    #[validate(length(max = 10), custom(function = "no_duplicates"))]
    pub tag_ids: Vec<u32>,
}

#[derive(Debug, Deserialize, Validate)]
pub struct PatchArticle {
    #[validate(length(min = 8, max = 200))]
    pub title: Option<String>,
    #[validate(length(min = 3, max = 200), custom(function = "slug_format"))]
    pub slug: Option<String>,
    #[validate(length(min = 20, max = 500))]
    pub excerpt: Option<String>,
    #[validate(length(min = 50))]
    pub body: Option<String>,
    #[validate(custom(function = "lang_code"))]
    pub lang: Option<String>,
    pub category_id: Option<u32>,
    pub author_id: Option<u32>,
    #[validate(length(max = 10), custom(function = "no_duplicates"))]
    pub tag_ids: Option<Vec<u32>>,
}

#[derive(Debug, Deserialize)]
pub struct PublishRequest {
    pub published_at: String,
}

#[derive(Debug, Deserialize)]
pub struct RunRequest {
    pub source: String,
    pub started_at: String,
    pub finished_at: String,
    pub found: u64,
    pub ingested: u64,
    pub errors: u64,
}

/// `Box<RawValue>` keeps each item as an unparsed slice so one bad item cannot
/// take the whole batch down, without building a `serde_json::Value` tree for
/// all 100 of them. `web::Json` needs `DeserializeOwned`, so the slices cannot
/// borrow from the payload and each item costs one allocation.
#[derive(Debug, Deserialize)]
pub struct BulkRequest {
    pub items: Vec<Box<serde_json::value::RawValue>>,
}

// --- custom validators -----------------------------------------------------

/// `^[a-z0-9]+(-[a-z0-9]+)*$`, spelled out rather than pulling in `regex`.
fn slug_format(value: &str) -> Result<(), ValidationError> {
    let ok = !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
        && value.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok { Ok(()) } else { Err(ValidationError::new("pattern")) }
}

fn lang_code(value: &str) -> Result<(), ValidationError> {
    if LANGS.contains(&value) { Ok(()) } else { Err(ValidationError::new("one_of")) }
}

fn no_duplicates(value: &[u32]) -> Result<(), ValidationError> {
    let mut seen = Vec::with_capacity(value.len());
    for id in value {
        if seen.contains(id) {
            return Err(ValidationError::new("duplicate"));
        }
        seen.push(*id);
    }
    Ok(())
}

// --- validator -> FieldError ----------------------------------------------

fn param_len(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::String(s)) => s.chars().count() as u64,
        Some(Value::Array(a)) => a.len() as u64,
        _ => 0,
    }
}

/// `validator` reports every length violation under the single code `length`
/// with min/max/value in `params`, so the direction has to be recovered here to
/// produce `min_length` / `max_length`.
fn from_validator(field: &str, err: &ValidationError) -> FieldError {
    let min = err.params.get("min").and_then(Value::as_u64);
    let max = err.params.get("max").and_then(Value::as_u64);
    let len = param_len(err.params.get("value"));
    let unit = match err.params.get("value") {
        Some(Value::Array(_)) => "entries",
        _ => "characters",
    };

    match &*err.code {
        "length" if min.is_some_and(|min| len < min) => FieldError::new(
            field,
            "min_length",
            format!("must be at least {} {unit}", min.unwrap_or(0)),
        ),
        "length" => FieldError::new(
            field,
            "max_length",
            format!("must be at most {} {unit}", max.unwrap_or(0)),
        ),
        "pattern" => FieldError::new(
            field,
            "pattern",
            "must match ^[a-z0-9]+(-[a-z0-9]+)*$".to_string(),
        ),
        "one_of" => {
            FieldError::new(field, "one_of", format!("must be one of {}", LANGS.join(", ")))
        }
        "duplicate" => {
            FieldError::new(field, "duplicate", "must not contain duplicates".to_string())
        }
        other => FieldError::new(field, "invalid", format!("failed check `{other}`")),
    }
}

/// `field_errors()` hands back a `HashMap`, so the output is sorted to keep the
/// response deterministic across runs.
pub fn collect(errors: &ValidationErrors) -> Vec<FieldError> {
    let mut out: Vec<FieldError> = errors
        .field_errors()
        .iter()
        .flat_map(|(field, errs)| errs.iter().map(move |err| from_validator(field, err)))
        .collect();
    out.sort_by(|a, b| a.field.cmp(&b.field).then_with(|| a.code.cmp(b.code)));
    out
}

// --- referential checks ----------------------------------------------------

/// Everything that needs the store rather than the value alone. Appends to
/// `out` so it composes with the `validator` output for one combined 422.
pub fn check_references(
    store: &Store,
    category_id: Option<u32>,
    author_id: Option<u32>,
    tag_ids: Option<&[u32]>,
    out: &mut Vec<FieldError>,
) {
    if let Some(id) = category_id
        && store.category(id).is_none()
    {
        out.push(FieldError::new("category_id", "not_found", format!("no category with id {id}")));
    }
    if let Some(id) = author_id
        && store.author(id).is_none()
    {
        out.push(FieldError::new("author_id", "not_found", format!("no author with id {id}")));
    }
    if let Some(ids) = tag_ids {
        for id in ids {
            if store.tag(*id).is_none() {
                out.push(FieldError::new("tag_ids", "not_found", format!("no tag with id {id}")));
            }
        }
    }
}

pub fn validate_create(store: &Store, input: &CreateArticle) -> Vec<FieldError> {
    let mut errors = match input.validate() {
        Ok(()) => Vec::new(),
        Err(err) => collect(&err),
    };
    check_references(
        store,
        Some(input.category_id),
        Some(input.author_id),
        Some(&input.tag_ids),
        &mut errors,
    );
    errors
}
