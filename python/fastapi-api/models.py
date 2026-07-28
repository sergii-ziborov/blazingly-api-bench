"""Wire models for the contract in ``SPEC.md``.

Every shape the API accepts or returns is declared exactly once here: the same
classes drive request validation, response serialisation and the generated
OpenAPI document.
"""

from datetime import datetime
from typing import Annotated, Any, Generic, Literal, TypeVar

from pydantic import AfterValidator, AwareDatetime, BaseModel, Field, ValidationInfo, field_validator
from pydantic_core import PydanticCustomError

# --------------------------------------------------------------------------
# Shared scalar types
# --------------------------------------------------------------------------

Lang = Literal["uk", "ru", "en"]
Stage = Literal["seed", "series_a", "series_b", "growth"]

SLUG_PATTERN = r"^[a-z0-9]+(-[a-z0-9]+)*$"


def _reject_duplicates(value: list[int]) -> list[int]:
    if len(set(value)) != len(value):
        raise PydanticCustomError("duplicate_items", "must not contain duplicate ids")
    return value


# Constrained aliases, declared once and reused by both the create and the
# patch payloads so the rules cannot drift between them.
Title = Annotated[str, Field(min_length=8, max_length=200)]
Slug = Annotated[str, Field(min_length=3, max_length=200, pattern=SLUG_PATTERN)]
Excerpt = Annotated[str, Field(min_length=20, max_length=500)]
Body = Annotated[str, Field(min_length=50)]
TagIds = Annotated[list[int], Field(max_length=10), AfterValidator(_reject_duplicates)]


# --------------------------------------------------------------------------
# Public read models
# --------------------------------------------------------------------------


class Ref(BaseModel):
    """A category, tag or author as embedded in an article."""

    id: int
    slug: str
    name: str


class ArticleSummary(BaseModel):
    id: int
    slug: str
    title: str
    excerpt: str
    lang: Lang
    # ``None`` while an article is still a draft: POST /admin/articles creates
    # drafts and POST /admin/articles/{id}/publish is what sets this.
    published_at: datetime | None
    reading_minutes: int
    views: int
    category: Ref
    author: Ref
    tags: list[Ref]
    cover_url: str


class ArticleDetail(ArticleSummary):
    body: str
    updated_at: datetime
    related: list[ArticleSummary]


class Company(BaseModel):
    id: int
    slug: str
    name: str
    industry: str
    stage: Stage
    founded_year: int
    employees: int
    total_funding_usd: int
    website: str


class TaxonomyItem(BaseModel):
    """A category or a tag in the flat listing endpoints."""

    id: int
    slug: str
    name: str
    article_count: int


class AuthorProfile(BaseModel):
    id: int
    slug: str
    name: str
    bio: str
    article_count: int


ItemT = TypeVar("ItemT")


class Page(BaseModel, Generic[ItemT]):
    items: list[ItemT]
    page: int
    limit: int
    total: int
    pages: int


# Concrete parametrisations. FastAPI needs the parametrised class both as the
# ``response_model`` and as the class the handler instantiates: an instance of
# the bare generic ``Page`` is not an instance of ``Page[ArticleSummary]`` and
# would fail response validation.
ArticlePage = Page[ArticleSummary]
CompanyPage = Page[Company]


class SearchResults(BaseModel):
    query: str
    articles: list[ArticleSummary]
    companies: list[Company]


# --------------------------------------------------------------------------
# Editorial write models
# --------------------------------------------------------------------------


class CreateArticle(BaseModel):
    title: Title
    slug: Slug
    excerpt: Excerpt
    body: Body
    lang: Lang
    category_id: int
    author_id: int
    tag_ids: TagIds = []


class UpdateArticle(BaseModel):
    """PATCH payload: every field optional, same rules when present.

    Handlers use ``model_fields_set`` to tell "absent" from "explicitly null".
    """

    title: Title | None = None
    slug: Slug | None = None
    excerpt: Excerpt | None = None
    body: Body | None = None
    lang: Lang | None = None
    category_id: int | None = None
    author_id: int | None = None
    tag_ids: TagIds | None = None


class PublishRequest(BaseModel):
    published_at: AwareDatetime

    @field_validator("published_at")
    @classmethod
    def _not_far_future(cls, value: datetime) -> datetime:
        from datetime import timedelta, timezone

        if value > datetime.now(timezone.utc) + timedelta(days=365):
            raise PydanticCustomError(
                "too_far_in_future",
                "must not be more than one year in the future",
            )
        return value


class CoverUploaded(BaseModel):
    id: int
    cover_url: str
    bytes: int
    content_type: str


# --------------------------------------------------------------------------
# Ingestion models
# --------------------------------------------------------------------------


class FieldError(BaseModel):
    """One validation failure, used both in error bodies and in bulk results."""

    field: str
    code: str
    message: str


class BulkIngest(BaseModel):
    # Deliberately untyped items: a ``list[CreateArticle]`` would reject the
    # whole batch on the first bad item, and the contract wants each item
    # reported individually. The schema reference restores what the annotation
    # gives up, so the OpenAPI document still documents the item shape.
    items: Annotated[
        list[Any],
        Field(
            min_length=1,
            max_length=100,
            json_schema_extra={"items": {"$ref": "#/components/schemas/CreateArticle"}},
        ),
    ]


class BulkItemResult(BaseModel):
    index: int
    status: Literal["created", "rejected", "duplicate"]
    id: int | None = None
    slug: str | None = None
    errors: list[FieldError] | None = None


class BulkResponse(BaseModel):
    accepted: int
    rejected: int
    results: list[BulkItemResult]


class IngestRunRequest(BaseModel):
    source: Annotated[str, Field(min_length=1, max_length=200)]
    started_at: AwareDatetime
    finished_at: AwareDatetime
    found: Annotated[int, Field(ge=0)]
    ingested: Annotated[int, Field(ge=0)]
    errors: Annotated[int, Field(ge=0)]

    @field_validator("finished_at")
    @classmethod
    def _after_start(cls, value: datetime, info: ValidationInfo) -> datetime:
        # Fields validate in declaration order, so ``started_at`` is already in
        # ``info.data`` — unless it failed, in which case it is absent and this
        # cross-field rule simply does not apply.
        started_at = info.data.get("started_at")
        if started_at is not None and value < started_at:
            raise PydanticCustomError(
                "before_started_at", "must not precede started_at"
            )
        return value


class IngestRun(IngestRunRequest):
    id: int


# --------------------------------------------------------------------------
# Operational models
# --------------------------------------------------------------------------


class HealthStatus(BaseModel):
    status: Literal["ok"]
    articles: int
    uptime_seconds: int
