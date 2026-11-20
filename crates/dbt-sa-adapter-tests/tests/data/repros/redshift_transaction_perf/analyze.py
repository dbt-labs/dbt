"""Summarize results.jsonl from run_experiment.py.

usage: analyze.py results.jsonl [REFERENCE_ARM]    (default reference: A)

Prints, per arm, the median job wall time and per-node time over clean runs, and then each arm's
per-node time against the reference arm of the same block (paired), with a Wilcoxon signed-rank
p-value when scipy is installed. With n blocks the smallest possible two-sided p is 2 / 2**n
(0.0312 for n = 6), so p only tells you the arms never swapped order.
"""

from __future__ import annotations

import json
import statistics as st
import sys
from pathlib import Path
from typing import Any

NAMES = {
    "A": "A  Fusion, default",
    "B": "B  Fusion, redshift_use_transactions",
    "E": "E  Fusion, comments-only transaction",
    "P": "P  second Fusion binary",
    "V1": "V1 python dbt-redshift",
}


def load(path: str) -> list[dict[str, Any]]:
    rows = [
        json.loads(line) for line in Path(path).read_text().splitlines() if line.strip()
    ]
    return [r for r in rows if not r.get("meta")]


def median_of(rows: list[dict[str, Any]], key: str) -> float:
    return st.median(r[key] for r in rows)


def main() -> None:
    rows = load(sys.argv[1])
    ref = sys.argv[2] if len(sys.argv) > 2 else "A"
    clean = [r for r in rows if r["rc"] == 0 and r["nodes_err"] == 0]
    arms = sorted({r["arm"] for r in rows})
    print(
        f"runs: {len(rows)} total, {len(clean)} clean, {len(rows) - len(clean)} with failures"
    )
    for r in rows:
        if r not in clean:
            print(
                f"  FAILED: arm {r['arm']} block {r['block']}: {r.get('output_tail', '')[-300:]!r}"
            )

    print(
        f"\n{'arm':38}{'n':>3} | {'job wall (s)':>12} | {'node mean (s)':>13} | {'node p95 (s)':>12} | {'commits':>7} {'queue ms':>8}"
    )
    for arm in arms:
        mine = [r for r in clean if r["arm"] == arm]
        if not mine:
            continue
        print(
            f"{NAMES.get(arm, arm):38}{len(mine):3d} | {median_of(mine, 'wall'):12.1f} | {median_of(mine, 'node_mean'):13.1f} | {median_of(mine, 'node_p95'):12.1f} | "
            f"{int(median_of(mine, 'commits')):7d} {int(median_of(mine, 'commit_queue_avg_ms')):8d}"
        )

    by_block: dict[int, dict[str, dict[str, Any]]] = {}
    for r in clean:
        by_block.setdefault(r["block"], {})[r["arm"]] = r
    print(f"\nper-node mean against {ref}, same block (ratio < 1 is faster)")
    try:
        from scipy.stats import wilcoxon
    except ImportError:
        wilcoxon = None
    for arm in arms:
        if arm == ref:
            continue
        pairs = [
            (b[ref]["node_mean"], b[arm]["node_mean"])
            for b in by_block.values()
            if ref in b and arm in b
        ]
        if len(pairs) < 3:
            continue
        ratios = [y / x for x, y in pairs]
        p = (
            f"p={wilcoxon([x for x, _ in pairs], [y for _, y in pairs]).pvalue:.4f}"
            if wilcoxon
            else "p=n/a (no scipy)"
        )
        print(
            f"  {NAMES.get(arm, arm):38} median {st.median(ratios):.2f}x  range {min(ratios):.2f}..{max(ratios):.2f}  {p}  blocks={len(pairs)}"
        )

    print("\nFusion statement means: comment_on / column lookup")
    for arm in arms:
        mine = [
            r for r in clean if r["arm"] == arm and "comment_on" in r.get("stmts", {})
        ]
        if mine:
            co = st.median(r["stmts"]["comment_on"]["mean"] for r in mine)
            lk = st.median(
                r["stmts"].get("lookup", {}).get("mean", float("nan")) for r in mine
            )
            print(
                f"  {NAMES.get(arm, arm):38} comment_on {co:5.2f}s   lookup {lk:5.2f}s"
            )

    sessions = [r["pre_other_sessions"] for r in rows]
    print(
        f"\nother sessions on the cluster before each run (noise): median {st.median(sessions)}, max {max(sessions)}"
    )


if __name__ == "__main__":
    main()
