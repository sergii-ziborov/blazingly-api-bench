"""In-memory store loaded from ``data/seed.json``.

Reads dominate, so the seed is turned into ready-to-serve Pydantic models once
at startup and the listing endpoints only slice pre-ordered lists. Writes are
rare and pay for keeping those lists ordered.
"""

import json
import math
import os
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Sequence

from models import (
    ArticleDetail,
    ArticleSummary,
    AuthorProfile,
    Company,
    CreateArticle,
    FieldError,
    IngestRun,
    IngestRunRequest,
    Ref,
    TaxonomyItem,
)

SEED_ENV_VAR = "BLAZINGLY_APIBENCH_SEED"
# This file lives at <repo>/python/fastapi-api/store.py, so the seed is two
# directories up. Resolved from __file__ rather than the process working
# directory so the server can be started from anywhere.
DEFAULT_SEED_PATH = Path(__file__).resolve().parent.parent.parent / "data" / "seed.json"

_EMPTY: list["ArticleRecord"] = []


def seed_path() -> Path:
    override = os.environ.get(SEED_ENV_VAR)
    return Path(override) if override else DEFAULT_SEED_PATH


def _utc_now() -> datetime:
    return datetime.now(timezone.utc).replace(microsecond=0)


def _reading_minutes(body: str) -> int:
    return max(1, math.ceil(len(body.split()) / 200))


# eq=False: records are compared by identity, so list.remove() on delete does
# not walk every field of every article looking for a value match.
@dataclass(slots=True, eq=False)
class ArticleRecord:
    """An article plus the denormalised keys the query paths need.

    ``summary`` is a fully built model instance, shared by every response that
    mentions this article.
    """

    id: int
    slug: str
    lang: str
    published_at: datetime | None
    updated_at: datetime
    body: str
    category_slug: str
    author_slug: str
    tag_slugs: frozenset[str]
    search_text: str
    summary: ArticleSummary

    @property
    def sort_key(self) -> tuple[datetime, int]:
        # Drafts have no published_at; they sort below everything published.
        return (self.published_at or datetime.min.replace(tzinfo=timezone.utc), self.id)


@dataclass(slots=True)
class Store:
    articles: list[ArticleRecord] = field(default_factory=list)
    by_id: dict[int, ArticleRecord] = field(default_factory=dict)
    by_slug: dict[str, ArticleRecord] = field(default_factory=dict)
    by_category: dict[str, list[ArticleRecord]] = field(default_factory=dict)
    by_tag: dict[str, list[ArticleRecord]] = field(default_factory=dict)
    by_author: dict[str, list[ArticleRecord]] = field(default_factory=dict)
    by_lang: dict[str, list[ArticleRecord]] = field(default_factory=dict)

    categories: list[Ref] = field(default_factory=list)
    tags: list[Ref] = field(default_factory=list)
    authors: list[Ref] = field(default_factory=list)
    category_by_id: dict[int, Ref] = field(default_factory=dict)
    tag_by_id: dict[int, Ref] = field(default_factory=dict)
    author_by_id: dict[int, Ref] = field(default_factory=dict)
    author_bio: dict[str, str] = field(default_factory=dict)
    author_by_slug: dict[str, Ref] = field(default_factory=dict)

    companies: list[Company] = field(default_factory=list)
    company_search: list[tuple[str, Company]] = field(default_factory=list)

    runs: list[IngestRun] = field(default_factory=list)
    _next_article_id: int = 1
    _next_run_id: int = 1

    # -- reads -------------------------------------------------------------

    @property
    def article_count(self) -> int:
        return len(self.articles)

    def newest(self) -> ArticleSummary:
        return self.articles[0].summary

    def filter_articles(
        self,
        *,
        category: str | None = None,
        tag: str | None = None,
        author: str | None = None,
        lang: str | None = None,
        q: str | None = None,
    ) -> Sequence[ArticleRecord]:
        """Articles matching every supplied filter, in the contract's order.

        Starts from the narrowest matching index and applies the rest as
        predicates, so ``?category=startups&lang=uk`` scans ~80 records rather
        than the full thousand.
        """
        indexes: list[tuple[str, list[ArticleRecord]]] = []
        if category is not None:
            indexes.append(("category", self.by_category.get(category, _EMPTY)))
        if tag is not None:
            indexes.append(("tag", self.by_tag.get(tag, _EMPTY)))
        if author is not None:
            indexes.append(("author", self.by_author.get(author, _EMPTY)))
        if lang is not None:
            indexes.append(("lang", self.by_lang.get(lang, _EMPTY)))

        if indexes:
            narrowest, records = min(indexes, key=lambda pair: len(pair[1]))
        else:
            narrowest, records = "", self.articles

        if category is not None and narrowest != "category":
            records = [r for r in records if r.category_slug == category]
        if tag is not None and narrowest != "tag":
            records = [r for r in records if tag in r.tag_slugs]
        if author is not None and narrowest != "author":
            records = [r for r in records if r.author_slug == author]
        if lang is not None and narrowest != "lang":
            records = [r for r in records if r.lang == lang]
        if q:
            needle = q.casefold()
            records = [r for r in records if needle in r.search_text]
        return records

    def search(self, q: str, limit: int = 10) -> tuple[list[ArticleSummary], list[Company]]:
        needle = q.casefold()
        articles: list[ArticleSummary] = []
        for record in self.articles:
            if needle in record.search_text:
                articles.append(record.summary)
                if len(articles) == limit:
                    break
        companies: list[Company] = []
        for text, company in self.company_search:
            if needle in text:
                companies.append(company)
                if len(companies) == limit:
                    break
        return articles, companies

    def filter_companies(
        self,
        *,
        industry: str | None = None,
        stage: str | None = None,
        min_funding: int | None = None,
    ) -> Sequence[Company]:
        if industry is None and stage is None and min_funding is None:
            return self.companies
        return [
            company
            for company in self.companies
            if (industry is None or company.industry == industry)
            and (stage is None or company.stage == stage)
            and (min_funding is None or company.total_funding_usd >= min_funding)
        ]

    def build_detail(self, record: ArticleRecord) -> ArticleDetail:
        related: list[ArticleSummary] = []
        for other in self.by_category.get(record.category_slug, _EMPTY):
            if other.id != record.id:
                related.append(other.summary)
                if len(related) == 3:
                    break
        # dict(model) keeps the nested Ref instances as-is; model_dump() would
        # flatten them to dicts only for Pydantic to rebuild them here.
        return ArticleDetail(
            **dict(record.summary),
            body=record.body,
            updated_at=record.updated_at,
            related=related,
        )

    def category_listing(self) -> list[TaxonomyItem]:
        return [
            TaxonomyItem(
                id=ref.id,
                slug=ref.slug,
                name=ref.name,
                article_count=len(self.by_category.get(ref.slug, _EMPTY)),
            )
            for ref in self.categories
        ]

    def tag_listing(self) -> list[TaxonomyItem]:
        return [
            TaxonomyItem(
                id=ref.id,
                slug=ref.slug,
                name=ref.name,
                article_count=len(self.by_tag.get(ref.slug, _EMPTY)),
            )
            for ref in self.tags
        ]

    def author_profile(self, slug: str) -> AuthorProfile | None:
        ref = self.author_by_slug.get(slug)
        if ref is None:
            return None
        return AuthorProfile(
            id=ref.id,
            slug=ref.slug,
            name=ref.name,
            bio=self.author_bio[slug],
            article_count=len(self.by_author.get(slug, _EMPTY)),
        )

    # -- validation that needs the store -----------------------------------

    def reference_errors(self, payload: CreateArticle) -> list[FieldError]:
        errors: list[FieldError] = []
        if payload.category_id not in self.category_by_id:
            errors.append(
                FieldError(
                    field="category_id",
                    code="not_found",
                    message=f"category {payload.category_id} does not exist",
                )
            )
        if payload.author_id not in self.author_by_id:
            errors.append(
                FieldError(
                    field="author_id",
                    code="not_found",
                    message=f"author {payload.author_id} does not exist",
                )
            )
        for index, tag_id in enumerate(payload.tag_ids):
            if tag_id not in self.tag_by_id:
                errors.append(
                    FieldError(
                        field=f"tag_ids.{index}",
                        code="not_found",
                        message=f"tag {tag_id} does not exist",
                    )
                )
        return errors

    # -- writes ------------------------------------------------------------

    def create_article(self, payload: CreateArticle) -> ArticleRecord:
        """Insert a draft. Callers must have validated references first."""
        article_id = self._next_article_id
        self._next_article_id += 1
        now = _utc_now()
        summary = ArticleSummary(
            id=article_id,
            slug=payload.slug,
            title=payload.title,
            excerpt=payload.excerpt,
            lang=payload.lang,
            published_at=None,
            reading_minutes=_reading_minutes(payload.body),
            views=0,
            category=self.category_by_id[payload.category_id],
            author=self.author_by_id[payload.author_id],
            tags=[self.tag_by_id[tag_id] for tag_id in sorted(payload.tag_ids)],
            cover_url=f"https://cdn.example/covers/{article_id:04d}.jpg",
        )
        record = ArticleRecord(
            id=article_id,
            slug=payload.slug,
            lang=payload.lang,
            published_at=None,
            updated_at=now,
            body=payload.body,
            category_slug=summary.category.slug,
            author_slug=summary.author.slug,
            tag_slugs=frozenset(tag.slug for tag in summary.tags),
            search_text=f"{payload.title}\n{payload.excerpt}".casefold(),
            summary=summary,
        )
        # A draft's sort key is below every published article and its id is the
        # highest issued so far, so appending keeps every list ordered.
        self._link(record)
        return record

    def update_article(self, record: ArticleRecord, changes: dict[str, object]) -> None:
        summary = record.summary
        if "title" in changes:
            summary.title = str(changes["title"])
        if "excerpt" in changes:
            summary.excerpt = str(changes["excerpt"])
        if "body" in changes:
            record.body = str(changes["body"])
            summary.reading_minutes = _reading_minutes(record.body)
        if "slug" in changes:
            del self.by_slug[record.slug]
            record.slug = summary.slug = str(changes["slug"])
            self.by_slug[record.slug] = record
        if "lang" in changes:
            record.lang = summary.lang = str(changes["lang"])  # type: ignore[assignment]
        if "category_id" in changes:
            summary.category = self.category_by_id[int(changes["category_id"])]  # type: ignore[arg-type]
            record.category_slug = summary.category.slug
        if "author_id" in changes:
            summary.author = self.author_by_id[int(changes["author_id"])]  # type: ignore[arg-type]
            record.author_slug = summary.author.slug
        if "tag_ids" in changes:
            tag_ids = sorted(changes["tag_ids"])  # type: ignore[call-overload]
            summary.tags = [self.tag_by_id[tag_id] for tag_id in tag_ids]
            record.tag_slugs = frozenset(tag.slug for tag in summary.tags)
        record.search_text = f"{summary.title}\n{summary.excerpt}".casefold()
        record.updated_at = _utc_now()
        self._reindex()

    def publish_article(self, record: ArticleRecord, published_at: datetime) -> None:
        record.published_at = published_at
        record.summary.published_at = published_at
        record.updated_at = _utc_now()
        self._reindex()

    def delete_article(self, record: ArticleRecord) -> None:
        del self.by_id[record.id]
        del self.by_slug[record.slug]
        self.articles.remove(record)
        self._reindex()

    def record_run(self, payload: IngestRunRequest) -> IngestRun:
        run = IngestRun(id=self._next_run_id, **payload.model_dump())
        self._next_run_id += 1
        self.runs.append(run)
        return run

    # -- indexing ----------------------------------------------------------

    def _link(self, record: ArticleRecord) -> None:
        self.by_id[record.id] = record
        self.by_slug[record.slug] = record
        self.articles.append(record)
        self.by_category.setdefault(record.category_slug, []).append(record)
        self.by_author.setdefault(record.author_slug, []).append(record)
        self.by_lang.setdefault(record.lang, []).append(record)
        for tag_slug in record.tag_slugs:
            self.by_tag.setdefault(tag_slug, []).append(record)

    def _reindex(self) -> None:
        """Rebuild every ordered index. Only writes that can reorder pay this."""
        self.articles.sort(key=lambda record: record.sort_key, reverse=True)
        self.by_category = {ref.slug: [] for ref in self.categories}
        self.by_tag = {ref.slug: [] for ref in self.tags}
        self.by_author = {ref.slug: [] for ref in self.authors}
        self.by_lang = {}
        for record in self.articles:
            self.by_category.setdefault(record.category_slug, []).append(record)
            self.by_author.setdefault(record.author_slug, []).append(record)
            self.by_lang.setdefault(record.lang, []).append(record)
            for tag_slug in record.tag_slugs:
                self.by_tag.setdefault(tag_slug, []).append(record)


def load_store(path: Path | None = None) -> Store:
    source = path or seed_path()
    with source.open("rb") as handle:
        payload = json.load(handle)

    store = Store()
    store.categories = [Ref(**item) for item in payload["categories"]]
    store.tags = [Ref(**item) for item in payload["tags"]]
    store.authors = [
        Ref(id=item["id"], slug=item["slug"], name=item["name"])
        for item in payload["authors"]
    ]
    store.category_by_id = {ref.id: ref for ref in store.categories}
    store.tag_by_id = {ref.id: ref for ref in store.tags}
    store.author_by_id = {ref.id: ref for ref in store.authors}
    store.author_by_slug = {ref.slug: ref for ref in store.authors}
    store.author_bio = {item["slug"]: item["bio"] for item in payload["authors"]}

    for item in payload["articles"]:
        category = store.category_by_id[item["category_id"]]
        author = store.author_by_id[item["author_id"]]
        tags = [store.tag_by_id[tag_id] for tag_id in item["tag_ids"]]
        summary = ArticleSummary(
            id=item["id"],
            slug=item["slug"],
            title=item["title"],
            excerpt=item["excerpt"],
            lang=item["lang"],
            published_at=item["published_at"],
            reading_minutes=item["reading_minutes"],
            views=item["views"],
            category=category,
            author=author,
            tags=tags,
            cover_url=item["cover_url"],
        )
        record = ArticleRecord(
            id=summary.id,
            slug=summary.slug,
            lang=summary.lang,
            published_at=summary.published_at,
            updated_at=datetime.fromisoformat(item["updated_at"]),
            body=item["body"],
            category_slug=category.slug,
            author_slug=author.slug,
            tag_slugs=frozenset(tag.slug for tag in tags),
            search_text=f"{summary.title}\n{summary.excerpt}".casefold(),
            summary=summary,
        )
        store.articles.append(record)
        store.by_id[record.id] = record
        store.by_slug[record.slug] = record

    # The seed is already in contract order, but sorting and indexing from the
    # sorted list keeps the invariant local to this module.
    store._reindex()
    store._next_article_id = max(store.by_id) + 1 if store.by_id else 1

    store.companies = [Company(**item) for item in payload["companies"]]
    store.company_search = [
        (f"{company.name}\n{company.industry}".casefold(), company)
        for company in store.companies
    ]
    return store
