//! Editorial endpoints.
//!
//! Auth is a plain function call at the top of each handler rather than an
//! extractor in the signature, because there is no signature machinery to hang
//! it off. Forgetting the call is the failure mode a framework protects you
//! from; here the router is the only caller and every arm is written out.

use bytes::Bytes;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use hyper::body::Incoming;
use hyper::{Response, StatusCode, header};
use serde::Serialize;

use crate::body::{self, ResBody};
use crate::error::ApiError;
use crate::store::{AppState, Article};
use crate::validate::{
    self, CreateArticle, MAX_UPLOAD_BYTES, PublishRequest, UPLOAD_HARD_LIMIT, UpdateArticle,
};
use crate::view;

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn reading_minutes(body: &str) -> u32 {
    (body.split_whitespace().count() as u32).div_ceil(200).max(1)
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

pub fn create_article(state: &AppState, payload: &Bytes) -> Result<Response<ResBody>, ApiError> {
    let input: CreateArticle = crate::router::from_json(payload)?;
    validate::fail_if(validate::shape_errors(&input))?;

    let mut articles = state.articles.write().expect("articles lock");
    validate::fail_if(validate::reference_errors(
        state,
        &articles,
        Some(input.category_id),
        Some(input.author_id),
        Some(&input.tag_ids),
        Some((input.slug.as_str(), None)),
    ))?;

    let id = articles.next_id();
    let article = build_article(id, &input, &now_rfc3339());
    let location = format!("/articles/{}", article.slug);
    articles.insert(article);

    let created = articles.get(id).ok_or(ApiError::Internal)?;
    let response = body::json(
        StatusCode::CREATED,
        &view::detail(state, &articles, created),
    );
    Ok(body::with_header(response, header::LOCATION, &location))
}

pub fn update_article(
    state: &AppState,
    id: u32,
    payload: &Bytes,
) -> Result<Response<ResBody>, ApiError> {
    let input: UpdateArticle = crate::router::from_json(payload)?;
    validate::fail_if(validate::update_shape_errors(&input))?;

    let mut articles = state.articles.write().expect("articles lock");
    if articles.get(id).is_none() {
        return Err(ApiError::NotFound("article"));
    }
    validate::fail_if(validate::reference_errors(
        state,
        &articles,
        input.category_id,
        input.author_id,
        input.tag_ids.as_deref(),
        input.slug.as_deref().map(|slug| (slug, Some(id))),
    ))?;

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
    Ok(body::ok(&view::detail(state, &articles, article)))
}

pub fn delete_article(state: &AppState, id: u32) -> Result<Response<ResBody>, ApiError> {
    let mut articles = state.articles.write().expect("articles lock");
    if articles.remove(id) {
        Ok(body::empty(StatusCode::NO_CONTENT))
    } else {
        Err(ApiError::NotFound("article"))
    }
}

pub fn publish_article(
    state: &AppState,
    id: u32,
    payload: &Bytes,
) -> Result<Response<ResBody>, ApiError> {
    let input: PublishRequest = crate::router::from_json(payload)?;
    if input.published_at.is_empty() {
        return Err(ApiError::field(
            "published_at",
            "min_length",
            "must be at least 1 characters",
        ));
    }
    let published_at = DateTime::parse_from_rfc3339(&input.published_at).map_err(|_| {
        ApiError::field("published_at", "format", "must be an RFC 3339 timestamp")
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
    Ok(body::ok(&view::detail(state, &articles, article)))
}

#[derive(Serialize)]
struct CoverAccepted<'a> {
    id: u32,
    cover_url: &'a str,
    bytes: usize,
    content_type: &'a str,
}

/// The one endpoint that has to touch the request body as a stream. There is no
/// `Multipart` extractor, so the boundary is pulled out of the content type by
/// hand and the body is handed to `multer` — the same parser axum's extractor
/// wraps — as a stream of data frames. Chunks are counted and dropped, so peak
/// RSS never carries the upload.
pub async fn upload_cover(
    state: &AppState,
    id: u32,
    content_type: Option<&str>,
    incoming: Incoming,
) -> Result<Response<ResBody>, ApiError> {
    {
        let articles = state.articles.read().expect("articles lock");
        if articles.get(id).is_none() {
            return Err(ApiError::NotFound("article"));
        }
    }

    let boundary = content_type
        .and_then(|value| multer::parse_boundary(value).ok())
        .ok_or_else(|| {
            ApiError::UnsupportedMediaType(
                "expected content-type: multipart/form-data with a boundary".to_owned(),
            )
        })?;

    let limited = http_body_util::Limited::new(incoming, UPLOAD_HARD_LIMIT);
    let stream = http_body_util::BodyDataStream::new(limited);
    let mut multipart = multer::Multipart::new(stream, boundary);

    while let Some(mut field) = multipart.next_field().await.map_err(multipart_error)? {
        if field.name() != Some("file") {
            continue;
        }
        let part_type = field
            .content_type()
            .map(|mime| mime.essence_str().to_owned())
            .unwrap_or_default();
        if part_type != "image/jpeg" && part_type != "image/png" {
            return Err(ApiError::UnsupportedMediaType(format!(
                "{part_type} is not an accepted cover type"
            )));
        }

        let mut bytes = 0_usize;
        while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
            bytes += chunk.len();
            if bytes > MAX_UPLOAD_BYTES {
                return Err(ApiError::PayloadTooLarge(format!(
                    "cover must not exceed {MAX_UPLOAD_BYTES} bytes"
                )));
            }
        }

        let articles = state.articles.read().expect("articles lock");
        let article = articles.get(id).ok_or(ApiError::NotFound("article"))?;
        return Ok(body::ok(&CoverAccepted {
            id: article.id,
            cover_url: &article.cover_url,
            bytes,
            content_type: &part_type,
        }));
    }

    Err(ApiError::field(
        "file",
        "required",
        "a multipart part named file is required",
    ))
}

fn multipart_error(error: multer::Error) -> ApiError {
    let text = error.to_string();
    if text.contains("length limit") {
        ApiError::PayloadTooLarge("request body exceeded the accepted size".to_owned())
    } else {
        ApiError::BadRequest(text)
    }
}
