"""Scraper ingestion surface. API-key auth plus a per-key rate limit."""

from fastapi import APIRouter, Depends, status
from pydantic import ValidationError

from dependencies import StoreDep, require_api_key
from errors import from_validation_error
from models import (
    BulkIngest,
    BulkItemResult,
    BulkResponse,
    CreateArticle,
    FieldError,
    IngestRun,
    IngestRunRequest,
)

router = APIRouter(
    prefix="/ingest",
    tags=["ingestion"],
    dependencies=[Depends(require_api_key)],
    responses={
        401: {"description": "Missing or invalid X-API-Key"},
        429: {"description": "Rate limit exceeded"},
    },
)


@router.post(
    "/articles/bulk",
    response_model=BulkResponse,
    response_model_exclude_none=True,
    summary="Ingest up to 100 articles, reporting each item separately",
)
async def bulk_ingest(payload: BulkIngest, store: StoreDep) -> BulkResponse:
    results: list[BulkItemResult] = []
    accepted = 0
    rejected = 0
    # Slugs created earlier in this same batch are duplicates too.
    for index, item in enumerate(payload.items):
        try:
            article = CreateArticle.model_validate(item)
        except ValidationError as exc:
            rejected += 1
            results.append(
                BulkItemResult(
                    index=index, status="rejected", errors=from_validation_error(exc)
                )
            )
            continue

        if article.slug in store.by_slug:
            rejected += 1
            results.append(
                BulkItemResult(index=index, status="duplicate", slug=article.slug)
            )
            continue

        errors: list[FieldError] = store.reference_errors(article)
        if errors:
            rejected += 1
            results.append(
                BulkItemResult(index=index, status="rejected", errors=errors)
            )
            continue

        record = store.create_article(article)
        accepted += 1
        results.append(
            BulkItemResult(
                index=index, status="created", id=record.id, slug=record.slug
            )
        )

    return BulkResponse(accepted=accepted, rejected=rejected, results=results)


@router.post("/runs", response_model=IngestRun, status_code=status.HTTP_201_CREATED)
async def record_run(payload: IngestRunRequest, store: StoreDep) -> IngestRun:
    return store.record_run(payload)
