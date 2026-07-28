"""Generates payloads/bulk50.json: 50 ingestion items, 10 of them invalid.

The invalid items are the point. A bulk endpoint that only ever sees valid
input measures serialization; one that has to validate, reject, and report per
item measures what the ingestion path actually costs. The ten failures cover a
different rule each time so no implementation can shortcut with a single early
return.
"""

from __future__ import annotations

import json
from pathlib import Path

VALID_BODY = (
    "Розробка платформи для аналітики даних у реальному часі, "
    "з інтеграцією у наявні системи замовника та підтримкою масштабування."
)


def valid_item(index: int) -> dict:
    return {
        "title": f"Стартап залучив раунд інвестицій номер {index:03d}",
        "slug": f"ingested-{index:04d}",
        "excerpt": "Компанія оголосила про залучення нового раунду фінансування.",
        "body": VALID_BODY,
        "lang": ["uk", "ru", "en"][index % 3],
        "category_id": (index % 12) + 1,
        "author_id": (index % 25) + 1,
        "tag_ids": [(index % 40) + 1, ((index + 7) % 40) + 1],
    }


def main() -> None:
    items = [valid_item(index) for index in range(50)]

    # Each invalid item violates exactly one rule, at a spread of positions.
    items[3]["title"] = "short"                       # title below 8 characters
    items[7]["slug"] = "Not A Valid Slug"             # slug pattern
    items[11]["excerpt"] = "too short"                # excerpt below 20
    items[16]["body"] = "tiny"                        # body below 50
    items[21]["lang"] = "de"                          # unsupported language
    items[26]["category_id"] = 999                    # unknown category
    items[31]["author_id"] = 999                      # unknown author
    items[36]["tag_ids"] = [1, 1]                     # duplicate tags
    items[41]["tag_ids"] = list(range(1, 13))         # more than 10 tags
    items[47]["slug"] = "article-0001"                # already exists in the seed

    payload = {"items": items}
    target = Path(__file__).resolve().parent.parent / "payloads" / "bulk50.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(
        json.dumps(payload, ensure_ascii=False, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    size = target.stat().st_size
    print(f"wrote {target}: {len(items)} items, 10 invalid, {size} bytes")


if __name__ == "__main__":
    main()
