# FastAPI implementation

The contract in [`SPEC.md`](../../SPEC.md), served on port **3205**.

```
main.py            app assembly, lifespan, exception handlers, body-size guard
models.py          every request and response shape (Pydantic v2)
store.py           seed loading and the in-memory, index-backed store
dependencies.py    store access, both auth schemes, the ingest rate limiter
errors.py          one error vocabulary for 422s and bulk item failures
routers/public.py     GET /articles, /categories, /tags, /authors, /companies, /search
routers/editorial.py  POST/PATCH/DELETE /admin/articles, cover upload, publish
routers/ingest.py     POST /ingest/articles/bulk, /ingest/runs
routers/ops.py        GET /health, GET /events (SSE)
```

## Setup

```powershell
uv venv .venv
uv pip install --python .venv/Scripts/python.exe -r requirements.txt
```

## Running

`--workers` is a uvicorn launcher flag, not something the app can set for
itself, so the worker count has to be passed on the command line. Read it from
`BLAZINGLY_BENCH_WORKERS`, defaulting to 4:

```powershell
$workers = if ($env:BLAZINGLY_BENCH_WORKERS) { $env:BLAZINGLY_BENCH_WORKERS } else { 4 }
.\.venv\Scripts\python.exe -m uvicorn main:app `
  --app-dir C:\path\to\blazingly-apibench\python\fastapi-api `
  --host 127.0.0.1 --port 3205 `
  --workers $workers --no-access-log --log-level warning
```

```bash
uvicorn main:app \
  --app-dir "$(dirname "$0")" \
  --host 127.0.0.1 --port 3205 \
  --workers "${BLAZINGLY_BENCH_WORKERS:-4}" --no-access-log --log-level warning
```

`--app-dir` puts this directory on `sys.path`, so the server can be started
from any working directory. `python main.py` also works and reads
`BLAZINGLY_BENCH_WORKERS` itself, but it is a convenience path only —
benchmarks should use the uvicorn command above.

`--no-access-log` matters: uvicorn's access log writes a formatted line per
request and costs measurable throughput.

## Seed data

Loaded once per worker process at startup, from `BLAZINGLY_APIBENCH_SEED` when
set and otherwise from `../../data/seed.json` resolved relative to `store.py`,
never relative to the working directory.

Each worker holds its own copy: ~1000 `ArticleSummary` models plus indexes.
Writes therefore only affect the worker that served them, which is why the
harness restarts the server for the mutating scenarios.

## Choices the contract leaves open

- **Error bodies.** Every 422 is `{"detail": [{"field", "code", "message"}]}`.
  The bulk endpoint has to report per-item errors in that shape, so request
  validation was translated into the same vocabulary rather than leaving
  FastAPI's default `{"type", "loc", "msg", "input"}` alongside it. Other
  statuses keep FastAPI's `{"detail": "..."}`.
- **`pages` when nothing matches** is `0`, not `1`.
- **Created articles are drafts.** `POST /admin/articles` returns
  `published_at: null`; `POST /admin/articles/{id}/publish` sets it. Otherwise
  the publish endpoint could only ever answer 409, since every seeded article
  is already published. Drafts sort below every published article, so they
  appear on the last page of `GET /articles`.
- **`GET /search`** matches article `title`/`excerpt` (as `?q=` does on
  `/articles`) and company `name`/`industry`, case-insensitively, capped at 10
  of each.
- **Role hierarchy.** `admin-token` satisfies routes that require `editor`;
  `editor-token` on the admin-only delete is 403.
- **Rate limiting** is a one-second fixed window per API key, per worker
  process. With N workers the effective budget is N x 100/s. Sharing it would
  need shared memory or Redis, which this benchmark deliberately excludes.
- **`POST /ingest/articles/bulk`** checks each item in this order: schema,
  then duplicate slug (against the store and against slugs created earlier in
  the same batch), then referential integrity. `accepted + rejected` covers
  every item; duplicates count as rejected.
