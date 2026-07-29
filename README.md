# blazingly-api-bench

The same API, written five times, measured and compared.

Most framework benchmarks measure a hello-world route, which tells you about a
router and nothing about a framework. This one implements a realistic content
API — public listings, an editorial admin surface, bulk ingestion from
scrapers, file upload, and a live event feed — in Blazingly, Axum, Actix Web,
bare hyper-on-Tokio, and FastAPI, and then measures both what it costs to run
and what it cost to write.

The Tokio implementation carries no web framework at all: routing, query
decoding, extractors, validation, error responses and SSE framing are written
out by hand. It is the floor the frameworks are measured against — what their
convenience costs relative to not having any.

The contract is [SPEC.md](SPEC.md). Every implementation serves it.

## Why both halves matter

Throughput alone rewards the framework that does least for you. Lines of code
alone rewards the framework that hides the most, including the parts you needed
to see. Reported together they say something useful: what a given amount of
convenience costs at runtime, and whether a given amount of speed costs you
anything at the keyboard.

Each implementation was written by someone using that framework's own idioms,
not by translating a common design, and each recorded the friction it hit while
writing. Those notes are as much the result as the numbers.

## Equivalence is checked before anything is measured

`tools/verify_equivalence.py` drives 24 cases against every running
implementation and compares parsed JSON, not bytes. A disagreement aborts the
run. Comparing the throughput of servers that return different things is not a
benchmark.

The gate passed for the run reported below: **120/120 checks agree** across all
five implementations.

Status codes are part of the contract. Error body shape is not: each framework
produces its idiomatic error format, because that format and what it costs to
produce is one of the things being compared.

## Running it

```powershell
.\run.ps1
```

That builds the Rust implementations, starts all five, verifies equivalence,
and then runs every scenario. Useful variations:

```powershell
.\run.ps1 -Scenario list -Rounds 5
.\run.ps1 -Framework tokio -SkipVerify
.\run.ps1 -Connections 128 -DurationSeconds 15 -Workers 8
.\run.ps1 -Scenario upload -Connections 8 -Rounds 5
```

`tools/analyze_run.py` turns a run log into per-scenario noise bands and
verdicts, which is what separates a difference from a spread:

```powershell
python tools\analyze_run.py results\<stamp>-run.log results\<stamp>-summary.txt
```

Samples are **interleaved**: every implementation runs round 1, then every
implementation runs round 2. On a machine carrying unrelated load this matters
more than the sample count, because background drift then hits all five rather
than whichever one happened to run during it. This suite exists partly because
an earlier non-interleaved run on this hardware produced a 23% "regression"
that turned out to be host noise.

A sample that records connection errors or unexpected statuses is **discarded
rather than averaged in**, and every discarded sample is listed at the bottom of
the report with its counts. A slow sample and a sample that answered wrongly are
not the same thing, and only one of them is a measurement.

## The load generator

`rust/loadgen` is repository-owned rather than off the shelf, for two reasons.
`go-wrk` reports every sub-millisecond round trip as `0s`, which erases the
percentile columns for the Rust servers entirely. And no common tool verifies
that the response was the expected status before counting it, so a server that
answers 500 quickly can win.

It keeps one request in flight per connection, discards a warm-up window, and
reports p50 through p99.9 with nanosecond resolution.

## Results

Run of 2026-07-29: 64 connections, 8s per sample, 2s warmup, 5 rounds, 4
workers, interleaved. Full write-up in
[`results/20260729-014235-analysis.md`](results/20260729-014235-analysis.md),
raw numbers in
[`results/20260729-014235-summary.txt`](results/20260729-014235-summary.txt).
This supersedes the 2026-07-28 run, which was taken at 85% background load and
before two framework fixes landed in `blazingly` (pattern matching, and keeping
uploads out of the JSON document).

**The `upload` rows were re-measured on 2026-07-29 after `blazingly-api`'s cover
handler was rewritten to stream**, at 8 and 32 connections, five rounds each,
interleaved, gate green at 120/120 both times. Write-up in
[`results/20260729-094851-analysis.md`](results/20260729-094851-analysis.md).
That pair was taken at **85.6% background CPU**, which is four times the
threshold this suite treats as reportable, so it re-measures the memory column
and leaves the throughput column unmeasured. The four implementations that did
not change reproduced their earlier peak RSS to within 5% while their throughput
fell two- to four-fold, which is what separates the two columns.

### The host was quieter than last time, but still not idle

| | avg | min | max |
|---|---|---|---|
| **Background CPU before the run** | **31.3%** | 25.7% | 37.9% |
| **Background CPU after the run** | **27.8%** | 18.8% | 38.5% |

Fourteen logical processors. The previous run was 85.2% before / 62.7% after and
drifted 22 points during the matrix; this one drifted 3.5.

That shows up directly in the noise. Repeating one implementation on one
scenario five times produced spreads of **1.33x to 2.04x**, against 1.27x–4.18x
before. More survives — but the band is still wider than every read-scenario
difference.

These figures are the 64-connection matrix only. The two `upload` re-runs were
taken later the same day at **85.6% before / 85.9% after**, on a host carrying
unrelated interactive load, with spreads up to 10.83x. Do not read the 31.3%
above onto the `upload` rows.

### Medians (true median of 5 rounds, requests/sec) with observed range

| scenario | blazingly | axum | actix | tokio | fastapi |
|---|---|---|---|---|---|
| **list** | 27,089 <br><sub>18.9k–31.4k</sub> | 19,538 <br><sub>15.8k–22.6k</sub> | 20,175 <br><sub>16.8k–25.1k</sub> | 19,530 <br><sub>17.3k–24.3k</sub> | 2,386 <br><sub>1.9k–2.7k</sub> |
| **detail** | 26,006 <br><sub>22.3k–29.7k</sub> | 22,363 <br><sub>21.3k–29.4k</sub> | 25,308 <br><sub>22.5k–33.4k</sub> | 27,066 <br><sub>22.3k–34.9k</sub> | 3,382 <br><sub>3.0k–4.5k</sub> |
| **filter** | 22,138 <br><sub>16.4k–25.4k</sub> | 19,155 <br><sub>13.2k–21.5k</sub> | 20,789 <br><sub>14.3k–24.8k</sub> | 15,506 <br><sub>13.9k–22.0k</sub> | 2,742 <br><sub>2.3k–3.4k</sub> |
| **search** | 27,171 <br><sub>18.6k–28.7k</sub> | 24,261 <br><sub>14.9k–26.1k</sub> | 26,015 <br><sub>14.5k–29.7k</sub> | 25,068 <br><sub>13.2k–26.9k</sub> | 3,094 <br><sub>2.9k–3.6k</sub> |
| **bulk** | 10,562 <br><sub>8.1k–11.6k</sub> | 9,021 <br><sub>6.0k–9.7k</sub> | 7,050 <br><sub>4.9k–8.3k</sub> | **11,963** <br><sub>8.5k–13.8k</sub> | 1,582 <br><sub>1.3k–2.0k</sub> |
| **upload** <br><sub>8 conn, superseded code</sub> | 352 <br><sub>347–378</sub> | 652 <br><sub>630–662</sub> | 669 <br><sub>659–671</sub> | 650 <br><sub>650–658</sub> | 64 <br><sub>58–67</sub> |
| **peak RSS** <br><sub>reads + bulk</sub> | 25–29 MiB | 25–27 MiB | 25–27 MiB | **24.5–26 MiB** | **297–303 MiB** |
| **peak RSS** <br><sub>upload, 8 conn</sub> | **25.9 MiB** | 34.4 MiB | 33.9 MiB | 36.6 MiB | 321.4 MiB |
| **peak RSS** <br><sub>upload, 32 conn</sub> | **25.9 MiB** | 66.0 MiB | 49.1 MiB | 76.5 MiB | 352.9 MiB |

`n=5` everywhere except FastAPI on `search` (`n=3`), and on `upload` where four
of five samples were lost at 8 connections (`n=1`) and one at 32 (`n=4`); actix
also lost one at 32 (`n=4`). All losses were connection errors, none were status
mismatches.

The `upload` throughput row is kept because it is the last measurement taken on
a quiet host, but for `blazingly` it measures **code that no longer exists** —
the buffered `File<UploadFile>` handler the streaming one replaced. Post-rewrite
upload throughput has not been measured on a host quiet enough to report.
The two `peak RSS` upload rows are from the post-rewrite runs, all five
implementations re-measured together.

### What these numbers support, and what they do not

| scenario | gap between the 4 Rust impls | noise band | verdict |
|---|---|---|---|
| list | 1.39x | 1.40x – 1.66x | **inside the noise** |
| detail | 1.21x | 1.33x – 1.56x | **inside the noise** |
| filter | 1.43x | 1.55x – 1.73x | **inside the noise** |
| search | 1.12x | 1.54x – 2.04x | **inside the noise** |
| **bulk** | **1.70x** | 1.43x – 1.69x | **real** — tokio over actix, no overlap |
| **upload** <br><sub>throughput, superseded code</sub> | **1.90x** | 1.01x – 1.09x | was **real** — actix over blazingly, no overlap — against the buffered handler since replaced |
| **upload** <br><sub>throughput, current code</sub> | 1.89x @ 8c, 1.94x @ 32c | 1.75x – 4.42x, 1.25x – 10.83x | **inside the noise** — unmeasured on this host |
| **upload** <br><sub>peak RSS, current code</sub> | **1.41x @ 8c, 2.95x @ 32c** | 1.00x – 1.05x | **real** — blazingly lowest of five, no sample overlap; 1.31x / 1.90x clear of the next-lowest |

Nothing is called real unless the leader's *worst* sample still beats the
trailer's *best*. With five rounds the true median is a single sample, so the
median-convention flip that decided two scenarios in the previous report cannot
occur here.

**Supported:**

- **All four Rust implementations beat FastAPI on every scenario**, by 8x–11x on
  reads and 6.7x on bulk, with no range overlap anywhere.
- **~11–12x less memory** on reads and bulk, in every sample, with almost no
  variance — resident memory does not care what else the host is doing.
- **`blazingly`'s bulk-ingest regression is fixed.** Median 782 → 10,562 rps and
  CPU 117% → 356% between the two runs. The quieter host lifted blazingly's read
  scenarios by 1.17x–1.97x and bulk by 13.5x, so roughly 8x of the bulk gain is
  not explained by the host; the CPU figure corroborates it independently. It is
  now second of four on bulk, and its worst sample is 4.1x FastAPI's best, where
  the two previously interleaved completely.
- **Bare `tokio` is fastest on bulk** — its worst sample (8,495) beats actix's
  best (8,328). Against blazingly and axum the ranges overlap, so those are
  directional only.
- **`blazingly`'s upload memory no longer scales with in-flight requests.** Its
  cover handler now reads the body as a stream, counting and dropping each chunk
  rather than receiving the whole part materialized. Peak RSS is **25.9 MiB at
  both 8 and 32 connections — 1.00x across a fourfold rise in concurrency**, with
  all ten samples inside 25.8–26.1 MiB. Against the buffered handler measured by
  this same harness that is 106.6 → 25.9 MiB at 8 connections (4.1x) and
  300.4 → 25.9 MiB at 32 (11.6x), where the buffered one had grown 2.82x between
  the two counts.
- **It is now the lowest peak RSS of the five on this scenario, at both
  connection counts**, and the only one that does not move with concurrency:
  25.9 MiB against actix 33.9 / axum 34.4 / tokio 36.6 at 8 connections, and
  against actix 49.1 / axum 66.0 / tokio 76.5 at 32. No sample overlap in either
  direction — its worst sample beats every other implementation's best. The gap
  widens with concurrency because theirs grows and its does not. The other three
  were never buffering the whole body either; what changed is the working set
  held per in-flight upload.

**Not supported — still do not read a ranking into the read scenarios:**

The four Rust implementations remain closer to each other than each is to
itself, and on no read scenario does any implementation's worst sample beat
another's best. `list`, `detail`, `filter` and `search` are **four Rust
implementations, one undifferentiated group, unmeasured on this host.**

This needs saying plainly because the table is suggestive: **blazingly has the
highest median on three of the four read scenarios.** That is a change from the
previous run and a reason to re-run on an idle machine. It is not a result.

Also unsupported: that blazingly beats axum or actix on bulk (ranges overlap);
and that actix's low bulk CPU (209%) means anything yet.

**And unsupported on uploads: any throughput comparison at all, post-rewrite.**
Both re-runs land inside the noise band — one implementation measured five times
disagreed with itself by up to 10.83x, more than any two implementations
disagreed with each other. The streaming rewrite may have helped upload
throughput, hurt it, or left it alone; this run cannot tell, and the drop in the
numbers is fully consistent with a host at 85.6% background CPU against the
20.5% the pre-change run enjoyed. What the last quiet-host measurement showed is
that the *buffered* handler was genuinely behind — 1.90x at 8 connections, 1.41x
at 32, non-overlapping. **Whether that gap survived the rewrite is open, and
needs a re-run on an idle machine.** The memory result above does not depend on
this, because the four unchanged implementations reproduced their earlier peak
RSS within 5% on the same loaded host.

### FastAPI's numbers flatter it

Three of 155 samples were discarded, all FastAPI, all connection errors with
**zero status mismatches** (uvicorn dropping keep-alive connections under load).
The discarded samples are the ones where it was under the most stress, so its
medians here are **optimistic**. Every Rust sample completed clean. This is a
large improvement on the previous run, which lost 7 of 20 FastAPI samples.

## Data

`data/seed.json` is generated by `tools/generate_seed.py` from a fixed PRNG seed
and committed, so all five load byte-identical data: 1000 articles, 200
companies, 25 authors, 40 tags, 12 categories.

`payloads/bulk50.json` is 50 ingestion items of which 10 are invalid, each
breaking a different rule. That mix is deliberate: a bulk endpoint fed only
valid input measures serialization, while one that must validate, reject, and
report per item measures what ingestion actually costs.

`payloads/cover5mib.bin` is a complete 5 MiB `multipart/form-data` body with a
fixed boundary, generated by `tools/generate_cover_payload.py`. The load
generator sends it verbatim, so all five receive identical bytes. Peak RSS on
this scenario is the point of it; measure at more than one connection count,
because buffering and streaming only diverge as concurrency rises.

## What this suite does not measure

- **A database.** All five use an in-memory store. Measuring a database would
  measure drivers and query planners. It also removes the thing FastAPI users
  genuinely suffer from in production, a synchronous driver blocking the event
  loop, so nothing here should be read as a claim about real deployments.
- **A network.** Client and server share a host over loopback.
- **An idle machine.** Every result records the host CPU at launch, and the
  report records it at both ends of the run. Treat any comparison taken above
  roughly 20% background load as directional only — the run published above was
  taken at 28–31%, which is why the read scenarios are still reported as
  unmeasured rather than as a ranking. Only differences that clear the noise
  band *and* show no sample overlap are stated as results.

Licensed under the MIT License.
