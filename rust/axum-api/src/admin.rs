//! Editorial endpoints. Every one of them is behind an auth extractor, so the
//! role requirement is visible in the handler signature.

use axum::extract::{Multipart, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::Serialize;

use crate::dto::{
    CreateArticle, MAX_UPLOAD_BYTES, PublishRequest, UpdateArticle, lang_is_valid, slug_is_valid,
    tag_ids_are_unique,
};
use crate::error::{ApiError, FieldError};
use crate::extract::{RequireAdmin, RequireEditor, ValidJson};
use crate::state::{AppInner, AppState, Article, Articles};
use crate::view;

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// The rules that need the store: referential integrity plus slug uniqueness.
/// `validator` can only reach these through `ValidateArgs`, which does not
/// compose with a generic extractor, so they are written out.
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
        errors.push(FieldError::new(
            "slug",
            "duplicate",
            "slug already exists",
        ));
    }
    errors
}

pub fn build_article(id: u32, input: &CreateArticle, now: &str) -> Article {
    Article {
        id,
        slug: input.slug.clone(),
        title: input.title.clone(),
        excerpt: input.excerpt.clone(),
        body: input.body.clone(),
        lang: input.lang.clone(),
        published_at: None,
        updated_at: now.to_owned(),
        reading_minutes: reading_minutes(&input.body),
        views: 0,
        category_id: input.category_id,
        author_id: input.author_id,
        tag_ids: input.tag_ids.clone(),
        cover_url: format!("https://cdn.example/covers/{id:04}.jpg"),
        haystack: format!("{} {}", input.title, input.excerpt).to_lowercase(),
        deleted: false,
    }
}

fn reading_minutes(body: &str) -> u32 {
    (body.split_whitespace().count() as u32).div_ceil(200).max(1)
}

pub async fn create_article(
    _: RequireEditor,
    State(state): State<AppState>,
    ValidJson(input): ValidJson<CreateArticle>,
) -> Result<Response, ApiError> {
    let mut articles = state.articles.write().expect("articles lock");
    let errors = reference_errors(
        &state,
        &articles,
        Some(input.category_id),
        Some(input.author_id),
        Some(&input.tag_ids),
        Some((input.slug.as_str(), None)),
    );
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    let id = articles.next_id();
    let article = build_article(id, &input, &now_rfc3339());
    let location = format!("/articles/{}", article.slug);
    articles.insert(article);

    let created = articles.get(id).ok_or(ApiError::Internal)?;
    let body = serde_json::to_vec(&view::detail(&state, &articles, created))
        .map_err(|_| ApiError::Internal)?;

    Ok((
        StatusCode::CREATED,
        [
            (header::CONTENT_TYPE, "application/json".to_owned()),
            (header::LOCATION, location),
        ],
        body,
    )
        .into_response())
}

pub async fn update_article(
    _: RequireEditor,
    State(state): State<AppState>,
    Path(id): Path<u32>,
    ValidJson(input): ValidJson<UpdateArticle>,
) -> Result<Response, ApiError> {
    let mut articles = state.articles.write().expect("articles lock");
    if articles.get(id).is_none() {
        return Err(ApiError::NotFound("article"));
    }

    let mut errors = Vec::new();
    if let Some(slug) = &input.slug
        && !slug_is_valid(slug)
    {
        errors.push(FieldError::new(
            "slug",
            "pattern",
            "must match ^[a-z0-9]+(-[a-z0-9]+)*$",
        ));
    }
    if let Some(lang) = &input.lang
        && !lang_is_valid(lang)
    {
        errors.push(FieldError::new(
            "lang",
            "one_of",
            "must be one of uk, ru, en",
        ));
    }
    if let Some(tag_ids) = &input.tag_ids
        && !tag_ids_are_unique(tag_ids)
    {
        errors.push(FieldError::new(
            "tag_ids",
            "duplicate",
            "must not contain duplicate ids",
        ));
    }
    errors.extend(reference_errors(
        &state,
        &articles,
        input.category_id,
        input.author_id,
        input.tag_ids.as_deref(),
        input.slug.as_deref().map(|slug| (slug, Some(id))),
    ));
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    let now = now_rfc3339();
    let renamed = {
        let article = articles.get_mut(id).ok_or(ApiError::NotFound("article"))?;
        let mut renamed = None;
        if let Some(title) = input.title {
            article.title = title;
        }
        if let Some(excerpt) = input.excerpt {
            article.excerpt = excerpt;
        }
        if let Some(body) = input.body {
            article.reading_minutes = reading_minutes(&body);
            article.body = body;
        }
        if let Some(lang) = input.lang {
            article.lang = lang;
        }
        if let Some(category_id) = input.category_id {
            article.category_id = category_id;
        }
        if let Some(author_id) = input.author_id {
            article.author_id = author_id;
        }
        if let Some(tag_ids) = input.tag_ids {
            article.tag_ids = tag_ids;
        }
        if let Some(slug) = input.slug {
            renamed = Some((std::mem::replace(&mut article.slug, slug.clone()), slug));
        }
        article.haystack = format!("{} {}", article.title, article.excerpt).to_lowercase();
        article.updated_at = now;
        renamed
    };
    if let Some((old, new)) = renamed {
        articles.rename(id, &old, &new);
    }

    let article = articles.get(id).ok_or(ApiError::Internal)?;
    view::ok(&view::detail(&state, &articles, article))
}

pub async fn delete_article(
    _: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<u32>,
) -> Result<StatusCode, ApiError> {
    let mut articles = state.articles.write().expect("articles lock");
    if articles.remove(id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound("article"))
    }
}

#[derive(Serialize)]
struct CoverAccepted<'a> {
    id: u32,
    cover_url: &'a str,
    bytes: usize,
    content_type: &'a str,
}

pub async fn upload_cover(
    _: RequireEditor,
    State(state): State<AppState>,
    Path(id): Path<u32>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    {
        let articles = state.articles.read().expect("articles lock");
        if articles.get(id).is_none() {
            return Err(ApiError::NotFound("article"));
        }
    }

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|error| ApiError::BadRequest(error.body_text()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let content_type = field.content_type().unwrap_or_default().to_owned();
        if content_type != "image/jpeg" && content_type != "image/png" {
            return Err(ApiError::UnsupportedMediaType(format!(
                "{content_type} is not an accepted cover type"
            )));
        }

        // Counted and dropped a chunk at a time: nothing but the current chunk
        // is ever resident.
        let mut bytes = 0_usize;
        loop {
            let chunk = field.chunk().await.map_err(map_multipart_error)?;
            let Some(chunk) = chunk else { break };
            bytes += chunk.len();
            if bytes > MAX_UPLOAD_BYTES {
                return Err(ApiError::PayloadTooLarge(format!(
                    "cover must not exceed {MAX_UPLOAD_BYTES} bytes"
                )));
            }
        }

        let articles = state.articles.read().expect("articles lock");
        let article = articles.get(id).ok_or(ApiError::NotFound("article"))?;
        return view::ok(&CoverAccepted {
            id: article.id,
            cover_url: &article.cover_url,
            bytes,
            content_type: &content_type,
        });
    }

    Err(ApiError::field(
        "file",
        "required",
        "a multipart part named file is required",
    ))
}

fn map_multipart_error(error: axum::extract::multipart::MultipartError) -> ApiError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::PayloadTooLarge("request body exceeded the accepted size".to_owned())
    } else {
        ApiError::BadRequest(error.body_text())
    }
}

pub async fn publish_article(
    _: RequireEditor,
    State(state): State<AppState>,
    Path(id): Path<u32>,
    ValidJson(input): ValidJson<PublishRequest>,
) -> Result<Response, ApiError> {
    let published_at = DateTime::parse_from_rfc3339(&input.published_at).map_err(|_| {
        ApiError::field(
            "published_at",
            "format",
            "must be an RFC 3339 timestamp",
        )
    })?;
    if published_at.with_timezone(&Utc) > Utc::now() + Duration::days(365) {
        return Err(ApiError::field(
            "published_at",
            "too_far_ahead",
            "must not be more than one year in the future",
        ));
    }

    let mut articles = state.articles.write().expect("articles lock");
    {
        let article = articles.get_mut(id).ok_or(ApiError::NotFound("article"))?;
        if article.published_at.is_some() {
            return Err(ApiError::Conflict("article is already published"));
        }
        article.published_at = Some(
            published_at
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Secs, true),
        );
        article.updated_at = now_rfc3339();
    }
    articles.reorder(id);

    let article = articles.get(id).ok_or(ApiError::Internal)?;
    view::ok(&view::detail(&state, &articles, article))
}
