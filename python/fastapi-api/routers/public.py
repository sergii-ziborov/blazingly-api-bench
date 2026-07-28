"""Public read surface: articles, taxonomies, authors, companies, search."""

from typing import Annotated, Sequence, TypeVar

from fastapi import APIRouter, HTTPException, Path, Query, status

from dependencies import StoreDep
from models import (
    ArticleDetail,
    ArticlePage,
    AuthorProfile,
    CompanyPage,
    Lang,
    SearchResults,
    Stage,
    TaxonomyItem,
)

router = APIRouter(tags=["public"])

PageNumber = Annotated[int, Query(ge=1, description="1-based page number")]
PageLimit = Annotated[int, Query(ge=1, le=100, description="Items per page")]

T = TypeVar("T")


def _window(items: Sequence[T], page: int, limit: int) -> tuple[Sequence[T], int, int]:
    total = len(items)
    pages = -(-total // limit)
    start = (page - 1) * limit
    return items[start : start + limit], total, pages


@router.get("/articles", response_model=ArticlePage, summary="List articles")
async def list_articles(
    store: StoreDep,
    page: PageNumber = 1,
    limit: PageLimit = 20,
    category: Annotated[str | None, Query(description="Category slug")] = None,
    tag: Annotated[str | None, Query(description="Tag slug")] = None,
    author: Annotated[str | None, Query(description="Author slug")] = None,
    lang: Annotated[Lang | None, Query()] = None,
    q: Annotated[str | None, Query(description="Matches title and excerpt")] = None,
) -> ArticlePage:
    records = store.filter_articles(
        category=category, tag=tag, author=author, lang=lang, q=q
    )
    window, total, pages = _window(records, page, limit)
    return ArticlePage(
        items=[record.summary for record in window],
        page=page,
        limit=limit,
        total=total,
        pages=pages,
    )


@router.get(
    "/articles/{slug}",
    response_model=ArticleDetail,
    responses={404: {"description": "No article with that slug"}},
)
async def get_article(store: StoreDep, slug: Annotated[str, Path()]) -> ArticleDetail:
    record = store.by_slug.get(slug)
    if record is None:
        raise HTTPException(status.HTTP_404_NOT_FOUND, f"No article with slug {slug!r}")
    return store.build_detail(record)


@router.get("/categories", response_model=list[TaxonomyItem])
async def list_categories(store: StoreDep) -> list[TaxonomyItem]:
    return store.category_listing()


@router.get("/tags", response_model=list[TaxonomyItem])
async def list_tags(store: StoreDep) -> list[TaxonomyItem]:
    return store.tag_listing()


@router.get(
    "/authors/{slug}",
    response_model=AuthorProfile,
    responses={404: {"description": "No author with that slug"}},
)
async def get_author(store: StoreDep, slug: Annotated[str, Path()]) -> AuthorProfile:
    profile = store.author_profile(slug)
    if profile is None:
        raise HTTPException(status.HTTP_404_NOT_FOUND, f"No author with slug {slug!r}")
    return profile


@router.get("/companies", response_model=CompanyPage)
async def list_companies(
    store: StoreDep,
    page: PageNumber = 1,
    limit: PageLimit = 20,
    industry: Annotated[str | None, Query()] = None,
    stage: Annotated[Stage | None, Query()] = None,
    min_funding: Annotated[int | None, Query(ge=0, description="USD")] = None,
) -> CompanyPage:
    companies = store.filter_companies(
        industry=industry, stage=stage, min_funding=min_funding
    )
    window, total, pages = _window(companies, page, limit)
    return CompanyPage(
        items=list(window), page=page, limit=limit, total=total, pages=pages
    )


@router.get("/search", response_model=SearchResults)
async def search(
    store: StoreDep,
    q: Annotated[str, Query(min_length=2, max_length=100)],
) -> SearchResults:
    articles, companies = store.search(q)
    return SearchResults(query=q, articles=articles, companies=companies)
