"""Shared dependencies: store access, the two auth schemes, the ingest budget."""

import math
import os
import time
from typing import Annotated, Literal

from fastapi import Depends, HTTPException, Request, status
from fastapi.security import APIKeyHeader, HTTPAuthorizationCredentials, HTTPBearer

from store import Store

Role = Literal["editor", "admin"]

# Fixed credentials, per the contract. Real verification is out of scope.
EDITORIAL_TOKENS: dict[str, Role] = {
    "editor-token": "editor",
    "admin-token": "admin",
}
INGEST_API_KEY = "scraper-key"
RANK: dict[Role, int] = {"editor": 1, "admin": 2}

# Requests per second per key. The contract fixes this at 100; the benchmark
# harness raises it for the bulk scenario, because otherwise that sample
# measures the rate limiter rather than validation. All four implementations
# read the same variable so the scenario stays comparable.
INGEST_RATE_LIMIT = int(os.environ.get("APIBENCH_INGEST_RPS", "100"))

# auto_error=False on both schemes: FastAPI's built-in security classes answer
# a missing credential with 403, and the contract wants 401. Turning the
# built-in error off is the only way to choose the status.
_bearer = HTTPBearer(auto_error=False, description="`editor-token` or `admin-token`")
_api_key = APIKeyHeader(name="X-API-Key", auto_error=False, description="`scraper-key`")


def get_store(request: Request) -> Store:
    return request.app.state.store


StoreDep = Annotated[Store, Depends(get_store)]


def require_role(minimum: Role):
    """Dependency factory: authenticate the editorial bearer token."""

    async def dependency(
        credentials: Annotated[
            HTTPAuthorizationCredentials | None, Depends(_bearer)
        ],
    ) -> Role:
        if credentials is None:
            raise HTTPException(
                status.HTTP_401_UNAUTHORIZED,
                "Missing editorial bearer token",
                headers={"WWW-Authenticate": "Bearer"},
            )
        role = EDITORIAL_TOKENS.get(credentials.credentials)
        if role is None:
            raise HTTPException(
                status.HTTP_401_UNAUTHORIZED,
                "Unknown editorial bearer token",
                headers={"WWW-Authenticate": "Bearer"},
            )
        if RANK[role] < RANK[minimum]:
            raise HTTPException(
                status.HTTP_403_FORBIDDEN,
                f"Role {role!r} may not perform this action; {minimum!r} required",
            )
        return role

    return dependency


Editor = Annotated[Role, Depends(require_role("editor"))]
Admin = Annotated[Role, Depends(require_role("admin"))]


class RateLimiter:
    """Fixed one-second window per key.

    Per process, so a multi-worker deployment enforces the budget per worker.
    A shared counter would need shared memory or Redis, which this benchmark
    deliberately does not have.
    """

    __slots__ = ("limit", "window", "_state")

    def __init__(self, limit: int, window: float = 1.0) -> None:
        self.limit = limit
        self.window = window
        self._state: dict[str, list[float]] = {}

    def hit(self, key: str) -> None:
        now = time.monotonic()
        entry = self._state.get(key)
        if entry is None or now - entry[0] >= self.window:
            self._state[key] = [now, 1]
            return
        entry[1] += 1
        if entry[1] > self.limit:
            retry_after = max(1, math.ceil(self.window - (now - entry[0])))
            raise HTTPException(
                status.HTTP_429_TOO_MANY_REQUESTS,
                f"Rate limit of {self.limit} requests per second exceeded",
                headers={"Retry-After": str(retry_after)},
            )


_rate_limiter = RateLimiter(INGEST_RATE_LIMIT)


async def require_api_key(
    api_key: Annotated[str | None, Depends(_api_key)],
) -> str:
    if api_key != INGEST_API_KEY:
        raise HTTPException(
            status.HTTP_401_UNAUTHORIZED, "Missing or invalid X-API-Key"
        )
    _rate_limiter.hit(api_key)
    return api_key


IngestKey = Annotated[str, Depends(require_api_key)]
