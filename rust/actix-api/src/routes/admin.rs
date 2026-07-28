//! Editorial surface, mounted under `web::scope("/admin")`.
//!
//! Authorisation is the `Editor` / `Admin` argument in each signature.

use actix_multipart::Multipart;
use actix_web::{HttpResponse, delete, patch, post, web};
use futures_util::StreamExt;
use serde::Serialize;

use crate::auth::{Admin, Editor};
use crate::clock;
use crate::dto;
use crate::error::{ApiError, FieldError};
use crate::store::{self, AppState, Article};
use crate::validation::{self, CreateArticle, PatchArticle, PublishRequest};

const MAX_COVER_BYTES: usize = 10 * 1024 * 1024;
const ALLOWED_COVER_TYPES: [&str; 2] = ["image/jpeg", "image/png"];

fn reading_minutes(body: &str) -> u32 {
    ((body.split_whitespace().count() as u32).div_ceil(200)).max(1)
}

pub fn new_article(id: u64, input: CreateArticle) -> Article {
    Article {
        id,
        reading_minutes: reading_minutes(&input.body),
        haystack: store::haystack(&input.title, &input.excerpt),
        slug: input.slug,
        title: input.title,
        excerpt: input.excerpt,
        body: input.body,
        lang: input.lang,
        published_at: None,
        updated_at: clock::now(),
        views: 0,
        category_id: input.category_id,
        author_id: input.author_id,
        tag_ids: input.tag_ids,
        cover_url: format!("https://cdn.example/covers/{id:04}.jpg"),
    }
}

#[post("/articles")]
async fn create_article(
    _editor: Editor,
    state: web::Data<AppState>,
    body: web::Json<CreateArticle>,
) -> Result<HttpResponse, ApiError> {
    let input = body.into_inner();
    let mut store = state.write();

    let mut errors = validation::validate_create(&store, &input);
    if store.article_by_slug(&input.slug).is_some() {
        errors.push(FieldError::new("slug", "duplicate", "slug already exists"));
    }
    if !errors.is_empty() {
        return Err(ApiError::invalid(errors));
    }

    let id = store.next_article_id();
    let slug = input.slug.clone();
    store.insert_article(new_article(id, input));

    let article = store.article_by_id(id).expect("just inserted");
    Ok(HttpResponse::Created()
        .insert_header(("location", format!("/articles/{slug}")))
        .json(dto::detail(&store, article)))
}

#[patch("/articles/{id}")]
async fn patch_article(
    _editor: Editor,
    state: web::Data<AppState>,
    path: web::Path<u64>,
    body: web::Json<PatchArticle>,
) -> Result<HttpResponse, ApiError> {
    let id = path.into_inner();
    let patch = body.into_inner();
    let mut store = state.write();

    if store.article_by_id(id).is_none() {
        return Err(ApiError::NotFound("article"));
    }

    let mut errors = match validator::Validate::validate(&patch) {
        Ok(()) => Vec::new(),
        Err(err) => validation::collect(&err),
    };
    validation::check_references(
        &store,
        patch.category_id,
        patch.author_id,
        patch.tag_ids.as_deref(),
        &mut errors,
    );
    if let Some(slug) = &patch.slug
        && store.article_by_slug(slug).is_some_and(|other| other.id != id)
    {
        errors.push(FieldError::new("slug", "duplicate", "slug already exists"));
    }
    if !errors.is_empty() {
        return Err(ApiError::invalid(errors));
    }

    store.update_article(id, |article| {
        if let Some(value) = patch.title {
            article.title = value;
        }
        if let Some(value) = patch.slug {
            article.slug = value;
        }
        if let Some(value) = patch.excerpt {
            article.excerpt = value;
        }
        if let Some(value) = patch.body {
            article.reading_minutes = reading_minutes(&value);
            article.body = value;
        }
        if let Some(value) = patch.lang {
            article.lang = value;
        }
        if let Some(value) = patch.category_id {
            article.category_id = value;
        }
        if let Some(value) = patch.author_id {
            article.author_id = value;
        }
        if let Some(value) = patch.tag_ids {
            article.tag_ids = value;
        }
        article.haystack = store::haystack(&article.title, &article.excerpt);
        article.updated_at = clock::now();
    });

    let article = store.article_by_id(id).expect("checked above");
    Ok(HttpResponse::Ok().json(dto::detail(&store, article)))
}

#[delete("/articles/{id}")]
async fn delete_article(
    _admin: Admin,
    state: web::Data<AppState>,
    path: web::Path<u64>,
) -> Result<HttpResponse, ApiError> {
    let mut store = state.write();
    if store.remove_article(path.into_inner()) {
        Ok(HttpResponse::NoContent().finish())
    } else {
        Err(ApiError::NotFound("article"))
    }
}

#[derive(Serialize)]
struct CoverResponse<'a> {
    id: u64,
    cover_url: &'a str,
    bytes: usize,
    content_type: &'a str,
}

#[post("/articles/{id}/cover")]
async fn upload_cover(
    _editor: Editor,
    state: web::Data<AppState>,
    path: web::Path<u64>,
    mut payload: Multipart,
) -> Result<HttpResponse, ApiError> {
    let id = path.into_inner();
    if state.read().article_by_id(id).is_none() {
        return Err(ApiError::NotFound("article"));
    }

    let mut uploaded: Option<(String, usize)> = None;

    while let Some(item) = payload.next().await {
        let mut field = item.map_err(|err| ApiError::BadRequest(format!("multipart: {err}")))?;
        if field.name() != Some("file") {
            while let Some(chunk) = field.next().await {
                chunk.map_err(|err| ApiError::BadRequest(format!("multipart: {err}")))?;
            }
            continue;
        }

        let content_type = field
            .content_type()
            .map(|mime| mime.essence_str().to_owned())
            .unwrap_or_else(|| "application/octet-stream".to_owned());
        if !ALLOWED_COVER_TYPES.contains(&content_type.as_str()) {
            return Err(ApiError::UnsupportedMediaType(content_type));
        }

        // Counted and dropped chunk by chunk: the 5 MiB body never lands in a
        // single buffer.
        let mut bytes = 0usize;
        while let Some(chunk) = field.next().await {
            let chunk = chunk.map_err(|err| ApiError::BadRequest(format!("multipart: {err}")))?;
            bytes += chunk.len();
            if bytes > MAX_COVER_BYTES {
                return Err(ApiError::PayloadTooLarge(MAX_COVER_BYTES));
            }
        }
        uploaded = Some((content_type, bytes));
    }

    let (content_type, bytes) =
        uploaded.ok_or_else(|| ApiError::invalid_field("file", "required", "missing file part"))?;

    let store = state.read();
    let article = store.article_by_id(id).ok_or(ApiError::NotFound("article"))?;
    Ok(HttpResponse::Ok().json(CoverResponse {
        id,
        cover_url: &article.cover_url,
        bytes,
        content_type: &content_type,
    }))
}

#[post("/articles/{id}/publish")]
async fn publish_article(
    _editor: Editor,
    state: web::Data<AppState>,
    path: web::Path<u64>,
    body: web::Json<PublishRequest>,
) -> Result<HttpResponse, ApiError> {
    let id = path.into_inner();
    let when = clock::parse(&body.published_at).ok_or_else(|| {
        ApiError::invalid_field("published_at", "format", "must be an RFC 3339 timestamp")
    })?;
    if clock::more_than_a_year_ahead(when) {
        return Err(ApiError::invalid_field(
            "published_at",
            "too_far_ahead",
            "must not be more than one year in the future",
        ));
    }

    let mut store = state.write();
    let article = store.article_by_id(id).ok_or(ApiError::NotFound("article"))?;
    if article.published_at.is_some() {
        return Err(ApiError::Conflict("article is already published"));
    }

    let stamp = clock::normalize(when);
    store.update_article(id, |article| {
        article.published_at = Some(stamp);
        article.updated_at = clock::now();
    });

    let article = store.article_by_id(id).expect("checked above");
    Ok(HttpResponse::Ok().json(dto::detail(&store, article)))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(create_article)
        .service(patch_article)
        .service(delete_article)
        .service(upload_cover)
        .service(publish_article);
}
