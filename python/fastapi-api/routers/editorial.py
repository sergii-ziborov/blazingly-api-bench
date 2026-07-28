"""Editorial surface. Bearer auth, role-gated per route."""

from typing import Annotated, Any

from fastapi import APIRouter, File, HTTPException, Response, UploadFile, status

from dependencies import Admin, Editor, StoreDep
from errors import ValidationFailed
from models import (
    ArticleDetail,
    CoverUploaded,
    CreateArticle,
    FieldError,
    PublishRequest,
    UpdateArticle,
)

router = APIRouter(prefix="/admin/articles", tags=["editorial"])

ALLOWED_COVER_TYPES = frozenset({"image/jpeg", "image/png"})
MAX_COVER_BYTES = 10 * 1024 * 1024
UPLOAD_CHUNK = 64 * 1024

UNAUTHORIZED = {401: {"description": "Missing or unknown bearer token"}}
FORBIDDEN = {403: {"description": "Role insufficient"}}
NOT_FOUND = {404: {"description": "No article with that id"}}


@router.post(
    "",
    response_model=ArticleDetail,
    status_code=status.HTTP_201_CREATED,
    responses={**UNAUTHORIZED, **FORBIDDEN},
    summary="Create a draft article",
)
async def create_article(
    payload: CreateArticle,
    role: Editor,
    store: StoreDep,
    response: Response,
) -> ArticleDetail:
    errors = store.reference_errors(payload)
    if payload.slug in store.by_slug:
        errors.append(
            FieldError(
                field="slug",
                code="duplicate",
                message=f"slug {payload.slug!r} already exists",
            )
        )
    if errors:
        raise ValidationFailed(errors)

    record = store.create_article(payload)
    response.headers["Location"] = f"/articles/{record.slug}"
    return store.build_detail(record)


@router.patch(
    "/{article_id}",
    response_model=ArticleDetail,
    responses={**UNAUTHORIZED, **FORBIDDEN, **NOT_FOUND},
)
async def update_article(
    article_id: int,
    payload: UpdateArticle,
    role: Editor,
    store: StoreDep,
) -> ArticleDetail:
    record = store.by_id.get(article_id)
    if record is None:
        raise HTTPException(status.HTTP_404_NOT_FOUND, f"No article with id {article_id}")

    # model_fields_set distinguishes "field omitted" from "field sent as null";
    # both mean "leave it alone" here, so nulls are dropped.
    changes: dict[str, Any] = {
        name: value
        for name, value in payload.model_dump(exclude_unset=True).items()
        if value is not None
    }
    if not changes:
        return store.build_detail(record)

    errors: list[FieldError] = []
    if "category_id" in changes and changes["category_id"] not in store.category_by_id:
        errors.append(
            FieldError(
                field="category_id",
                code="not_found",
                message=f"category {changes['category_id']} does not exist",
            )
        )
    if "author_id" in changes and changes["author_id"] not in store.author_by_id:
        errors.append(
            FieldError(
                field="author_id",
                code="not_found",
                message=f"author {changes['author_id']} does not exist",
            )
        )
    for index, tag_id in enumerate(changes.get("tag_ids", ())):
        if tag_id not in store.tag_by_id:
            errors.append(
                FieldError(
                    field=f"tag_ids.{index}",
                    code="not_found",
                    message=f"tag {tag_id} does not exist",
                )
            )
    if "slug" in changes:
        existing = store.by_slug.get(changes["slug"])
        if existing is not None and existing.id != record.id:
            errors.append(
                FieldError(
                    field="slug",
                    code="duplicate",
                    message=f"slug {changes['slug']!r} already exists",
                )
            )
    if errors:
        raise ValidationFailed(errors)

    store.update_article(record, changes)
    return store.build_detail(record)


@router.delete(
    "/{article_id}",
    status_code=status.HTTP_204_NO_CONTENT,
    responses={**UNAUTHORIZED, **FORBIDDEN, **NOT_FOUND},
    summary="Delete an article (admin only)",
)
async def delete_article(article_id: int, role: Admin, store: StoreDep) -> Response:
    record = store.by_id.get(article_id)
    if record is None:
        raise HTTPException(status.HTTP_404_NOT_FOUND, f"No article with id {article_id}")
    store.delete_article(record)
    return Response(status_code=status.HTTP_204_NO_CONTENT)


@router.post(
    "/{article_id}/cover",
    response_model=CoverUploaded,
    responses={
        **UNAUTHORIZED,
        **FORBIDDEN,
        **NOT_FOUND,
        413: {"description": "Larger than 10 MiB"},
        415: {"description": "Not image/jpeg or image/png"},
    },
    summary="Replace an article cover",
)
async def upload_cover(
    article_id: int,
    role: Editor,
    store: StoreDep,
    file: Annotated[UploadFile, File(description="JPEG or PNG, at most 10 MiB")],
) -> CoverUploaded:
    record = store.by_id.get(article_id)
    if record is None:
        raise HTTPException(status.HTTP_404_NOT_FOUND, f"No article with id {article_id}")

    content_type = (file.content_type or "").split(";", 1)[0].strip().lower()
    if content_type not in ALLOWED_COVER_TYPES:
        raise HTTPException(
            status.HTTP_415_UNSUPPORTED_MEDIA_TYPE,
            f"{content_type or 'unknown'} is not an accepted cover type",
        )

    # Starlette has already buffered the part (in memory up to 1 MiB, then a
    # temp file), so this loop counts rather than streams. The oversize guard
    # that runs before buffering lives in MaxBodySizeMiddleware; this one
    # catches requests that arrive without a Content-Length.
    size = 0
    while chunk := await file.read(UPLOAD_CHUNK):
        size += len(chunk)
        if size > MAX_COVER_BYTES:
            raise HTTPException(
                status.HTTP_413_REQUEST_ENTITY_TOO_LARGE,
                f"Cover exceeds {MAX_COVER_BYTES} bytes",
            )
    await file.close()

    return CoverUploaded(
        id=record.id,
        cover_url=record.summary.cover_url,
        bytes=size,
        content_type=content_type,
    )


@router.post(
    "/{article_id}/publish",
    response_model=ArticleDetail,
    responses={
        **UNAUTHORIZED,
        **FORBIDDEN,
        **NOT_FOUND,
        409: {"description": "Already published"},
    },
)
async def publish_article(
    article_id: int,
    payload: PublishRequest,
    role: Editor,
    store: StoreDep,
) -> ArticleDetail:
    record = store.by_id.get(article_id)
    if record is None:
        raise HTTPException(status.HTTP_404_NOT_FOUND, f"No article with id {article_id}")
    if record.published_at is not None:
        raise HTTPException(
            status.HTTP_409_CONFLICT, f"Article {article_id} is already published"
        )
    store.publish_article(record, payload.published_at)
    return store.build_detail(record)
