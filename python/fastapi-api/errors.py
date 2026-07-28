"""Error rendering.

The contract fixes status codes but leaves the error body to each framework.
This one keeps a single shape for every 422 — ``{"detail": [FieldError, ...]}``
— because the bulk ingestion endpoint has to report per-item field errors in
exactly that form, and having request validation produce something different
would mean two vocabularies for the same failure.

FastAPI's own 422 body is ``{"detail": [{"type", "loc", "msg", "input"}]}``, so
the handler below translates it once, in one place.
"""

from typing import Any, Iterable

from fastapi import Request
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse
from pydantic import ValidationError

from models import FieldError

# Pydantic error types are implementation vocabulary ("string_too_short");
# the contract's example uses rule names ("min_length"). Map the ones this API
# can actually produce and fall back to the raw pydantic type for the rest, so
# an unmapped case degrades to something truthful rather than to "unknown".
_CODES: dict[str, str] = {
    "missing": "required",
    "string_type": "type",
    "string_too_short": "min_length",
    "string_too_long": "max_length",
    "string_pattern_mismatch": "pattern",
    "int_type": "type",
    "int_parsing": "type",
    "float_parsing": "type",
    "bool_parsing": "type",
    "list_type": "type",
    "dict_type": "type",
    "model_type": "type",
    "model_attributes_type": "type",
    "too_short": "min_items",
    "too_long": "max_items",
    "literal_error": "invalid_value",
    "enum": "invalid_value",
    "greater_than_equal": "min_value",
    "less_than_equal": "max_value",
    "greater_than": "min_value",
    "less_than": "max_value",
    "datetime_parsing": "invalid_datetime",
    "datetime_type": "invalid_datetime",
    "datetime_from_date_parsing": "invalid_datetime",
    "timezone_aware": "invalid_datetime",
    "json_invalid": "invalid_json",
    "extra_forbidden": "unexpected_field",
    "value_error": "invalid_value",
}


class ValidationFailed(Exception):
    """422 for rules Pydantic cannot express.

    Referential integrity ("category_id must exist") needs the store, and the
    store is not reachable from a model validator during request parsing, so
    those checks run in the handler and raise this instead.
    """

    def __init__(self, errors: list[FieldError]) -> None:
        self.errors = errors
        super().__init__(f"{len(errors)} validation error(s)")


def _field_name(loc: tuple[Any, ...], *, drop_prefix: bool) -> str:
    """Turn a Pydantic error location into a field name.

    ``drop_prefix`` removes FastAPI's request-part marker (``body``, and only
    the first element). It must stay off when validating a value directly,
    because ``CreateArticle`` really does have a field called ``body`` and its
    error location is ``("body",)`` — indistinguishable from the marker unless
    the caller says which kind of error list this is.

    An empty location means the payload itself was wrong (a bulk item that is
    not an object, say); it is reported as the JSON-path root rather than as a
    name that could collide with a real field.
    """
    parts = list(loc)
    if drop_prefix and parts and parts[0] == "body":
        parts = parts[1:]
    return ".".join(str(part) for part in parts) if parts else "$"


def to_field_errors(
    errors: Iterable[dict[str, Any]], *, drop_prefix: bool = False
) -> list[FieldError]:
    return [
        FieldError(
            # A JSON syntax error locates a character offset, not a field, so
            # its location would otherwise be reported as a field named "17".
            field=(
                "$"
                if error["type"] == "json_invalid"
                else _field_name(error["loc"], drop_prefix=drop_prefix)
            ),
            code=_CODES.get(error["type"], error["type"]),
            message=error["msg"],
        )
        for error in errors
    ]


def from_validation_error(exc: ValidationError) -> list[FieldError]:
    """Field errors for a value validated by hand, e.g. one bulk item."""
    return to_field_errors(exc.errors(include_url=False, include_context=False))


def _body(errors: list[FieldError]) -> dict[str, Any]:
    return {"detail": [error.model_dump() for error in errors]}


async def request_validation_handler(
    request: Request, exc: RequestValidationError
) -> JSONResponse:
    return JSONResponse(
        status_code=422,
        content=_body(to_field_errors(exc.errors(), drop_prefix=True)),
    )


async def validation_failed_handler(
    request: Request, exc: ValidationFailed
) -> JSONResponse:
    return JSONResponse(status_code=422, content=_body(exc.errors))
