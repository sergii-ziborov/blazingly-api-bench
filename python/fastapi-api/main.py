"""FastAPI implementation of the blazingly-apibench contract (SPEC.md).

Run it with uvicorn; see README.md. Worker count is a launcher concern, so this
module only builds the ASGI app.
"""

import time
from contextlib import asynccontextmanager
from typing import AsyncIterator

from fastapi import FastAPI
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse
from starlette.types import ASGIApp, Receive, Scope, Send

from errors import (
    ValidationFailed,
    request_validation_handler,
    validation_failed_handler,
)
from routers import editorial, ingest, ops, public
from routers.editorial import MAX_COVER_BYTES
from store import load_store


class MaxBodySizeMiddleware:
    """Reject oversized uploads before the body is read.

    FastAPI has no declarative body-size limit, and dependencies do not help:
    the router parses the request body *before* it solves dependencies, so by
    the time any handler code runs the bytes have already been buffered. Raw
    ASGI is the only layer left that can answer 413 from the headers alone.
    """

    # Cap on how much of a rejected body we are willing to read and throw away
    # before giving up and letting the connection close.
    DRAIN_LIMIT = 64 * 1024 * 1024

    def __init__(self, app: ASGIApp, *, max_bytes: int, path_suffix: str) -> None:
        self.app = app
        self.max_bytes = max_bytes
        self.path_suffix = path_suffix

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] == "http" and scope["path"].endswith(self.path_suffix):
            for name, value in scope["headers"]:
                if name == b"content-length":
                    if value.isdigit() and int(value) > self.max_bytes:
                        await self._reject(scope, receive, send)
                        return
                    break
        await self.app(scope, receive, send)

    async def _reject(self, scope: Scope, receive: Receive, send: Send) -> None:
        # Answer 413 without ever holding the body, but read past it first:
        # a client still writing megabytes into the socket sees a reset rather
        # than the response if the server replies and closes immediately.
        drained = 0
        while drained < self.DRAIN_LIMIT:
            message = await receive()
            if message["type"] != "http.request":
                break
            drained += len(message.get("body", b""))
            if not message.get("more_body", False):
                break
        response = JSONResponse(
            {"detail": f"Body exceeds {self.max_bytes} bytes"}, status_code=413
        )
        await response(scope, receive, send)


@asynccontextmanager
async def lifespan(app: FastAPI) -> AsyncIterator[None]:
    app.state.store = load_store()
    app.state.started_at = time.monotonic()
    yield
    app.state.store = None


app = FastAPI(
    title="blazingly-apibench: FastAPI",
    version="1.0.0",
    summary="Tech-news content API with editorial and ingestion surfaces.",
    lifespan=lifespan,
)

app.add_middleware(
    MaxBodySizeMiddleware, max_bytes=MAX_COVER_BYTES, path_suffix="/cover"
)

app.add_exception_handler(RequestValidationError, request_validation_handler)
app.add_exception_handler(ValidationFailed, validation_failed_handler)

app.include_router(public.router)
app.include_router(editorial.router)
app.include_router(ingest.router)
app.include_router(ops.router)


if __name__ == "__main__":  # pragma: no cover - convenience for local runs
    import os

    import uvicorn

    uvicorn.run(
        "main:app",
        host="127.0.0.1",
        port=3205,
        workers=int(os.environ.get("BLAZINGLY_BENCH_WORKERS", "4")),
        log_level="warning",
        access_log=False,
    )
