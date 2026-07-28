"""Checks that the running implementations answer the contract identically.

This runs before any measurement. Comparing the throughput of two servers that
return different things is not a benchmark, it is a coincidence, so a
disagreement here fails the run rather than being reported as a result.

Comparison is over parsed JSON, not bytes: field order and whitespace are not
part of the contract. Status codes are.

Usage:
    python tools/verify_equivalence.py blazingly=3201 axum=3202 fastapi=3205
"""

from __future__ import annotations

import json
import sys
import urllib.error
import urllib.request
from typing import Any

TIMEOUT = 15

# (name, method, path, body, headers, expected_status)
CASES: list[tuple[str, str, str, Any, dict[str, str], int]] = [
    ("health", "GET", "/health", None, {}, 200),
    ("list_default", "GET", "/articles", None, {}, 200),
    ("list_page3", "GET", "/articles?page=3&limit=20", None, {}, 200),
    ("list_limit_max", "GET", "/articles?limit=100", None, {}, 200),
    ("list_bad_page", "GET", "/articles?page=0", None, {}, 422),
    ("list_bad_limit", "GET", "/articles?limit=101", None, {}, 422),
    ("list_bad_lang", "GET", "/articles?lang=de", None, {}, 422),
    ("filter_category", "GET", "/articles?category=startups&lang=uk&limit=20", None, {}, 200),
    ("filter_unknown_category", "GET", "/articles?category=nope", None, {}, 200),
    ("filter_tag", "GET", "/articles?tag=ai&limit=10", None, {}, 200),
    ("detail", "GET", "/articles/article-0042", None, {}, 200),
    ("detail_missing", "GET", "/articles/does-not-exist", None, {}, 404),
    ("categories", "GET", "/categories", None, {}, 200),
    ("tags", "GET", "/tags", None, {}, 200),
    ("author", "GET", "/authors/author-07", None, {}, 200),
    ("author_missing", "GET", "/authors/nobody", None, {}, 404),
    ("companies", "GET", "/companies?page=2&limit=20", None, {}, 200),
    ("companies_filter", "GET", "/companies?stage=series_a&min_funding=1000000", None, {}, 200),
    ("search", "GET", "/search?q=ai", None, {}, 200),
    ("search_short", "GET", "/search?q=a", None, {}, 422),
    ("search_missing", "GET", "/search", None, {}, 422),
    ("admin_no_auth", "POST", "/admin/articles", {"title": "x"}, {}, 401),
    ("admin_bad_token", "POST", "/admin/articles", {"title": "x"},
     {"Authorization": "Bearer nope"}, 401),
    ("ingest_no_key", "POST", "/ingest/articles/bulk", {"items": []}, {}, 401),
]

# Fields whose values legitimately differ between servers and processes.
VOLATILE = {"uptime_seconds"}


def request(port: int, method: str, path: str, body: Any,
            headers: dict[str, str]) -> tuple[int, Any]:
    data = None
    send_headers = dict(headers)
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        send_headers["Content-Type"] = "application/json"
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}{path}", data=data, method=method,
        headers=send_headers,
    )
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as response:
            payload = response.read()
            status = response.status
    except urllib.error.HTTPError as error:
        payload = error.read()
        status = error.code
    try:
        return status, json.loads(payload) if payload else None
    except json.JSONDecodeError:
        return status, {"__unparsed__": payload[:200].decode("utf-8", "replace")}


def scrub(value: Any) -> Any:
    """Drops fields that cannot match across independent processes."""
    if isinstance(value, dict):
        return {k: scrub(v) for k, v in value.items() if k not in VOLATILE}
    if isinstance(value, list):
        return [scrub(item) for item in value]
    return value


def main() -> int:
    targets: dict[str, int] = {}
    for argument in sys.argv[1:]:
        name, _, port = argument.partition("=")
        targets[name] = int(port)
    if len(targets) < 2:
        print("need at least two targets to compare", file=sys.stderr)
        return 2

    reference_name = next(iter(targets))
    failures = 0

    for case_name, method, path, body, headers, expected in CASES:
        observed: dict[str, tuple[int, Any]] = {}
        for name, port in targets.items():
            observed[name] = request(port, method, path, body, headers)

        for name, (status, _) in observed.items():
            if status != expected:
                print(f"FAIL {case_name}: {name} returned {status}, contract says {expected}")
                failures += 1

        # Error bodies are deliberately not part of the contract, so only
        # successful responses are compared field by field.
        if expected < 400:
            reference = scrub(observed[reference_name][1])
            for name, (_, payload) in observed.items():
                if name == reference_name:
                    continue
                if scrub(payload) != reference:
                    print(f"FAIL {case_name}: {name} differs from {reference_name}")
                    left = json.dumps(reference, ensure_ascii=False, sort_keys=True)[:400]
                    right = json.dumps(scrub(payload), ensure_ascii=False, sort_keys=True)[:400]
                    print(f"  {reference_name}: {left}")
                    print(f"  {name}: {right}")
                    failures += 1

    total = len(CASES) * len(targets)
    if failures:
        print(f"\n{failures} equivalence failures across {total} checks")
        return 1
    print(f"all {total} checks agree across {', '.join(targets)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
