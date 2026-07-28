"""Generates data/seed.json.

Deterministic: a fixed PRNG seed and no dependency on dict iteration order, so
regenerating produces byte-identical output and every implementation loads the
same corpus. The result is committed; this script exists to make it auditable
and regenerable, not to run at startup.
"""

from __future__ import annotations

import json
import random
from datetime import datetime, timedelta, timezone
from pathlib import Path

SEED = 20260728
ARTICLES = 1000
CATEGORIES = 12
TAGS = 40
AUTHORS = 25
COMPANIES = 200

# Fixed epoch so published_at never depends on when this script runs.
EPOCH = datetime(2026, 7, 1, 12, 0, 0, tzinfo=timezone.utc)

CATEGORY_NAMES = [
    ("startups", "Стартапи"),
    ("tech", "Технології"),
    ("fintech", "Фінтех"),
    ("ai", "Штучний інтелект"),
    ("gamedev", "Геймдев"),
    ("crypto", "Криптовалюти"),
    ("hardware", "Залізо"),
    ("bigtech", "Великий тех"),
    ("careers", "Кар'єра"),
    ("investments", "Інвестиції"),
    ("cybersec", "Кібербезпека"),
    ("opinion", "Колонки"),
]

TAG_NAMES = [
    "ai", "llm", "saas", "b2b", "b2c", "seed-round", "series-a", "exit",
    "acquisition", "remote", "hiring", "layoffs", "open-source", "rust",
    "python", "javascript", "cloud", "devops", "security", "privacy",
    "regulation", "eu", "usa", "poland", "diia", "unicorn", "bootstrap",
    "accelerator", "vc", "angel", "hardware", "robotics", "drones", "defense",
    "medtech", "edtech", "agritech", "logistics", "marketplace", "mobile",
]

INDUSTRIES = [
    "fintech", "saas", "marketplace", "medtech", "edtech", "gamedev",
    "cybersec", "logistics", "agritech", "defense",
]
STAGES = ["seed", "series_a", "series_b", "growth"]
LANGS = ["uk", "ru", "en"]

TITLE_WORDS = [
    "стартап", "інвестиції", "раунд", "команда", "продукт", "ринок",
    "платформа", "сервіс", "додаток", "технологія", "модель", "дані",
    "клієнти", "виручка", "зростання", "фонд", "угода", "запуск",
]
BODY_WORDS = TITLE_WORDS + [
    "компанія", "засновник", "розробка", "аналітика", "інтеграція",
    "масштабування", "інфраструктура", "безпека", "автоматизація",
]


def sentence(rng: random.Random, words: list[str], count: int) -> str:
    picked = [rng.choice(words) for _ in range(count)]
    return " ".join(picked).capitalize()


def main() -> None:
    rng = random.Random(SEED)

    categories = [
        {"id": index + 1, "slug": slug, "name": name}
        for index, (slug, name) in enumerate(CATEGORY_NAMES[:CATEGORIES])
    ]
    tags = [
        {"id": index + 1, "slug": slug, "name": slug.replace("-", " ").title()}
        for index, slug in enumerate(TAG_NAMES[:TAGS])
    ]
    authors = [
        {
            "id": index + 1,
            "slug": f"author-{index + 1:02d}",
            "name": f"Автор {index + 1:02d}",
            "bio": sentence(rng, BODY_WORDS, 12),
        }
        for index in range(AUTHORS)
    ]

    articles = []
    for index in range(ARTICLES):
        article_id = index + 1
        category = rng.choice(categories)
        author = rng.choice(authors)
        tag_count = rng.randint(1, 5)
        article_tags = rng.sample(tags, tag_count)
        article_tags.sort(key=lambda tag: tag["id"])
        published = EPOCH - timedelta(minutes=37 * index)
        updated = published + timedelta(minutes=rng.randint(0, 600))
        articles.append(
            {
                "id": article_id,
                "slug": f"article-{article_id:04d}",
                "title": sentence(rng, TITLE_WORDS, rng.randint(4, 9)),
                "excerpt": sentence(rng, BODY_WORDS, rng.randint(14, 28)),
                "body": "\n\n".join(
                    sentence(rng, BODY_WORDS, rng.randint(25, 45))
                    for _ in range(rng.randint(4, 9))
                ),
                "lang": rng.choice(LANGS),
                "published_at": published.strftime("%Y-%m-%dT%H:%M:%SZ"),
                "updated_at": updated.strftime("%Y-%m-%dT%H:%M:%SZ"),
                "reading_minutes": rng.randint(2, 12),
                "views": rng.randint(120, 98000),
                "category_id": category["id"],
                "author_id": author["id"],
                "tag_ids": [tag["id"] for tag in article_tags],
                "cover_url": f"https://cdn.example/covers/{article_id:04d}.jpg",
            }
        )

    # The contract fixes the default ordering so no implementation can rely on
    # incidental map ordering.
    articles.sort(key=lambda item: (item["published_at"], item["id"]), reverse=True)

    companies = [
        {
            "id": index + 1,
            "slug": f"company-{index + 1:03d}",
            "name": f"Company {index + 1:03d}",
            "industry": rng.choice(INDUSTRIES),
            "stage": rng.choice(STAGES),
            "founded_year": rng.randint(2010, 2025),
            "employees": rng.randint(3, 1200),
            "total_funding_usd": rng.randrange(50_000, 250_000_000, 50_000),
            "website": f"https://company-{index + 1:03d}.example",
        }
        for index in range(COMPANIES)
    ]

    payload = {
        "categories": categories,
        "tags": tags,
        "authors": authors,
        "articles": articles,
        "companies": companies,
    }

    target = Path(__file__).resolve().parent.parent / "data" / "seed.json"
    target.write_text(
        json.dumps(payload, ensure_ascii=False, indent=1, sort_keys=False) + "\n",
        encoding="utf-8",
    )
    print(f"wrote {target} with {len(articles)} articles, {len(companies)} companies")


if __name__ == "__main__":
    main()
