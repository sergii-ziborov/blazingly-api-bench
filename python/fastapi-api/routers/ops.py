"""Operational surface: liveness and the live article feed."""

import asyncio
import time
from typing import AsyncIterator

from fastapi import APIRouter, Request
from fastapi.responses import EventSourceResponse
from fastapi.sse import ServerSentEvent

from dependencies import StoreDep
from models import HealthStatus

router = APIRouter(tags=["ops"])

FEED_INTERVAL_SECONDS = 1.0
HEARTBEAT_EVERY = 5


@router.get("/health", response_model=HealthStatus)
async def health(request: Request, store: StoreDep) -> HealthStatus:
    uptime = time.monotonic() - request.app.state.started_at
    return HealthStatus(
        status="ok", articles=store.article_count, uptime_seconds=int(uptime)
    )


@router.get(
    "/events",
    response_class=EventSourceResponse,
    summary="Newest article, once per second, as Server-Sent Events",
)
async def events(store: StoreDep) -> AsyncIterator[ServerSentEvent]:
    emitted = 0
    while True:
        yield ServerSentEvent(event="article", data=store.newest())
        emitted += 1
        if emitted % HEARTBEAT_EVERY == 0:
            yield ServerSentEvent(comment="heartbeat")
        await asyncio.sleep(FEED_INTERVAL_SECONDS)
