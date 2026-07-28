"""Turns a run log into a noise-aware verdict per scenario.

The summary the harness writes reports a median and a range. That is not enough
to say whether a difference between two implementations means anything, because
on a loaded host one implementation measured twice already differs. So this
computes, per scenario, the spread of each implementation across its own rounds
-- best sample over worst sample -- and compares the gap between implementations
against it.

Two tests are applied, and they are not the same strength:

  noise-band test  the gap between implementation medians against the widest
                   single-implementation spread in that scenario. Conservative:
                   a gap smaller than the noisiest implementation's own spread
                   is not evidence of anything.

  overlap test     whether the winner's worst sample still beats the loser's
                   best sample. This is the strong one. It does not depend on
                   the choice of median convention, and a difference that
                   survives it survived every sample that was taken.

A gap that fails the noise-band test is reported as unmeasured, not as a small
difference. Reading a ranking out of numbers that cannot support one is the
failure mode this whole suite exists to avoid.

Usage:
    python tools/analyze_run.py results/<timestamp>-run.log [results/<timestamp>-summary.txt]

Per-round samples live in the run log; the closing host-CPU reading lives only
in the summary, so pass both to get all of it.
"""

from __future__ import annotations

import re
import statistics
import sys
from collections import defaultdict
from pathlib import Path

SAMPLE = re.compile(
    r"^(?P<server>\S+)\s+(?P<scenario>\S+)\s+round\s+(?P<round>\d+)\s+"
    r"(?P<rps>[\d,]+)\s+rps\s+p50\s+(?P<p50>\S+)\s+p99\s+(?P<p99>\S+)\s+"
    r"cpu\s+(?P<cpu>-?[\d,]+)%\s+rss\s+(?P<rss>[\d.,]+)\s+MiB\s+"
    r"host\s+(?P<host>[\d.]+)%"
)
DISCARD = re.compile(
    r"(?P<server>\S+)/(?P<scenario>\S+)\s+round\s+(?P<round>\d+)\s+discarded:\s+"
    r"(?P<errors>\d+)\s+connection errors,\s+(?P<mismatches>\d+)\s+status mismatches"
)
HOST_CPU = re.compile(
    r"host (?:background )?CPU (?P<when>before|after)[^:]*:\s+avg\s+(?P<avg>[\d.]+)%\s+"
    r"min\s+(?P<min>[\d.]+)%\s+max\s+(?P<max>[\d.]+)%"
)

RUST = ("blazingly", "axum", "actix", "tokio")


def number(text: str) -> float:
    return float(text.replace(",", ""))


class Series:
    """Every accepted sample of one implementation on one scenario."""

    def __init__(self) -> None:
        self.rps: list[float] = []
        self.rss: list[float] = []
        self.cpu: list[float] = []
        self.p50: list[str] = []
        self.p99: list[str] = []

    @property
    def n(self) -> int:
        return len(self.rps)

    @property
    def median(self) -> float:
        return statistics.median(self.rps)

    @property
    def low(self) -> float:
        return min(self.rps)

    @property
    def high(self) -> float:
        return max(self.rps)

    @property
    def spread(self) -> float:
        """Best sample over worst. One implementation's disagreement with itself."""
        return self.high / self.low if self.low > 0 else float("inf")


def parse(paths: list[Path]) -> tuple[dict, list, dict]:
    series: dict[tuple[str, str], Series] = defaultdict(Series)
    discarded: list[dict] = []
    host: dict[str, dict[str, float]] = {}

    lines: list[str] = []
    for path in paths:
        lines.extend(path.read_text(encoding="utf-8", errors="replace").splitlines())

    for line in lines:
        stripped = line.strip()
        if match := SAMPLE.match(stripped):
            entry = series[(match["server"], match["scenario"])]
            entry.rps.append(number(match["rps"]))
            entry.rss.append(number(match["rss"]))
            entry.cpu.append(number(match["cpu"]))
            entry.p50.append(match["p50"])
            entry.p99.append(match["p99"])
        elif match := DISCARD.search(stripped):
            discarded.append(
                {
                    "server": match["server"],
                    "scenario": match["scenario"],
                    "round": int(match["round"]),
                    "errors": int(match["errors"]),
                    "mismatches": int(match["mismatches"]),
                }
            )
        elif match := HOST_CPU.search(stripped):
            host[match["when"]] = {
                "avg": float(match["avg"]),
                "min": float(match["min"]),
                "max": float(match["max"]),
            }
    return series, discarded, host


def scenario_order(series: dict) -> list[str]:
    seen: list[str] = []
    for _, scenario in series:
        if scenario not in seen:
            seen.append(scenario)
    return seen


def verdict(series: dict, scenario: str, group: tuple[str, ...]) -> dict:
    present = [name for name in group if series.get((name, scenario))]
    if len(present) < 2:
        return {}
    spreads = {name: series[(name, scenario)].spread for name in present}
    band_low, band_high = min(spreads.values()), max(spreads.values())

    ranked = sorted(present, key=lambda name: series[(name, scenario)].median, reverse=True)
    best, worst = ranked[0], ranked[-1]
    gap = series[(best, scenario)].median / series[(worst, scenario)].median

    # The strong test: does the leader's worst sample still beat the trailer's best?
    separated = series[(best, scenario)].low > series[(worst, scenario)].high

    return {
        "present": present,
        "ranked": ranked,
        "gap": gap,
        "band_low": band_low,
        "band_high": band_high,
        "beats_noise": gap > band_high,
        "separated": separated,
        "spreads": spreads,
    }


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    paths = [Path(argument) for argument in sys.argv[1:]]
    series, discarded, host = parse(paths)
    if not series:
        joined = ", ".join(str(path) for path in paths)
        print(f"no samples parsed from {joined}", file=sys.stderr)
        return 1

    for when in ("before", "after"):
        if when in host:
            values = host[when]
            print(
                f"host background CPU {when}: avg {values['avg']}%  "
                f"min {values['min']}%  max {values['max']}%"
            )
    print()

    servers = []
    for server, _ in series:
        if server not in servers:
            servers.append(server)

    outliers: list[str] = []
    for scenario in scenario_order(series):
        print(f"## {scenario}")
        print()
        print(
            f"{'impl':<10} {'n':>3} {'median rps':>12} {'min':>10} {'max':>10} "
            f"{'spread':>7} {'p50':>10} {'p99':>10} {'cpu%':>6} {'peak MiB':>9}"
        )
        for server in servers:
            entry = series.get((server, scenario))
            if not entry:
                print(f"{server:<10} {'--':>3}  no valid samples")
                continue
            middle = sorted(range(entry.n), key=lambda i: entry.rps[i])[entry.n // 2]
            # Median rather than max: the process-tree collector can attribute an
            # unrelated process's memory to a server that inherited a recycled
            # PID, and one such reading would otherwise become the headline.
            rss = statistics.median(entry.rss)
            flag = " *" if max(entry.rss) > 4 * rss else ""
            print(
                f"{server:<10} {entry.n:>3} {entry.median:>12,.0f} {entry.low:>10,.0f} "
                f"{entry.high:>10,.0f} {entry.spread:>6.2f}x {entry.p50[middle]:>10} "
                f"{entry.p99[middle]:>10} {statistics.median(entry.cpu):>6.0f} "
                f"{rss:>9.1f}{flag}"
            )
            if flag:
                outliers.append(
                    f"  {server:<10} {scenario:<7} peak RSS samples "
                    f"{', '.join(f'{value:,.1f}' for value in entry.rss)} MiB"
                )

        rust = verdict(series, scenario, RUST)
        if rust:
            print()
            print(
                f"  rust group: gap {rust['gap']:.2f}x "
                f"({rust['ranked'][0]} over {rust['ranked'][-1]}), "
                f"noise band {rust['band_low']:.2f}x-{rust['band_high']:.2f}x"
            )
            if rust["beats_noise"]:
                label = "REAL (ranges do not overlap)" if rust["separated"] else \
                    "exceeds the noise band, but sample ranges overlap: directional"
            else:
                label = "INSIDE THE NOISE - unmeasured on this host"
            print(f"  verdict: {label}")

        everyone = verdict(series, scenario, tuple(servers))
        if everyone and everyone["ranked"][-1] not in RUST:
            slowest = everyone["ranked"][-1]
            fastest = everyone["ranked"][0]
            gap = everyone["gap"]
            sep = series[(fastest, scenario)].low > series[(slowest, scenario)].high
            print(
                f"  {fastest} over {slowest}: {gap:.2f}x, "
                f"ranges {'do not overlap' if sep else 'overlap'}"
            )
        print()

    if outliers:
        print("peak-RSS outliers (marked * above; the median is reported instead):")
        for line in outliers:
            print(line)
        print()

    if discarded:
        print("discarded samples (excluded from every figure above):")
        for entry in discarded:
            print(
                f"  {entry['server']:<10} {entry['scenario']:<7} round {entry['round']}  "
                f"errors={entry['errors']} mismatches={entry['mismatches']}"
            )
    else:
        print("no samples were discarded: every sample completed with "
              "0 errors and 0 status mismatches")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
