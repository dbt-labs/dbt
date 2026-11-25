"""Run the Redshift transaction experiment: randomized blocks over several arms.

Arms (A, B and E are Fusion, one binary, three project variants; see gen_project.py):
  A   Fusion, default settings
  B   Fusion, `redshift_use_transactions: true`
  E   Fusion, a transaction around the column comments only
  P   a second Fusion binary (--pristine-bin) on project A. Use a build without your change to
      check that arm A matches it.
  V1  Python dbt-redshift (--v1-dbt) on project A, as the reference

Each block runs every arm once in random order, so slow drift on a shared cluster hits all arms
alike. Per-node times come from run_results.json (all arms). Per-statement times come from
Fusion's query_log.sql. Commit-queue stats come from stl_commit_stats, for the run's time window.

usage: run_experiment.py --v2-bin PATH [--arms A,B] [--threads 64] [--blocks 5] [--out results.jsonl]
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import random
import re
import shutil
import statistics as st
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

import common
import gen_project


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    return ordered[max(0, int(len(ordered) * fraction) - 1)]


def node_times(target_path: Path) -> tuple[list[float], int]:
    """Execution time of each succeeded node, and how many nodes did not succeed."""
    path = target_path / "run_results.json"
    if not path.exists():
        return [], 0
    results = json.loads(path.read_text())["results"]
    ok = [
        r["execution_time"] for r in results if r.get("status") in ("success", "pass")
    ]
    return ok, len(results) - len(ok)


def statement_stats(log_path: Path) -> dict[str, dict[str, float]]:
    """Per-kind statement timings from Fusion's query_log.sql."""
    path = log_path / "query_log.sql"
    if not path.exists():
        return {}
    timings: dict[str, list[float]] = {}
    for block in path.read_text().split("\n-- created_at:")[1:]:
        elapsed = re.search(r"elapsed: ([\d.]+)(ms|s|m)\b", block)
        if not elapsed or "-- desc:" not in block:
            continue
        seconds = (
            float(elapsed.group(1))
            * {"ms": 1e-3, "s": 1.0, "m": 60.0}[elapsed.group(2)]
        )
        sql = (
            re.sub(
                r"/\*.*?\*/",
                "",
                block.split("-- desc:")[1].split("\n", 1)[1],
                flags=re.DOTALL,
            )
            .strip()
            .lower()
        )
        if sql.startswith("comment on"):
            kind = "comment_on"
        elif "from information_schema" in sql and "columns" in sql:
            kind = "lookup"
        elif sql.startswith("begin"):
            kind = "begin"
        elif sql.startswith("commit"):
            kind = "commit"
        else:
            continue
        timings.setdefault(kind, []).append(seconds)
    return {
        kind: {
            "n": len(v),
            "mean": round(st.mean(v), 3),
            "p95": round(percentile(v, 0.95), 2),
            "total": round(sum(v), 1),
        }
        for kind, v in timings.items()
    }


class Experiment:
    def __init__(
        self,
        args: argparse.Namespace,
        work: Path,
        projects: dict[str, Path],
        profiles: Path,
    ) -> None:
        self.args, self.work, self.projects, self.profiles = (
            args,
            work,
            projects,
            profiles,
        )
        self.admin = common.connect(common.load_output()).cursor()

    def query(self, sql: str, params: tuple[Any, ...] = ()) -> list[Any]:
        self.admin.execute(sql, params)
        return list(self.admin.fetchall())

    def command(self, arm: str, run_dir: Path) -> tuple[list[str], Path]:
        a = self.args
        binary, project = {
            "A": (a.v2_bin, "A"),
            "B": (a.v2_bin, "B"),
            "E": (a.v2_bin, "E"),
            "P": (a.pristine_bin, "A"),
            "V1": (a.v1_dbt, "A"),
        }[arm]
        cmd = [
            binary,
            "run",
            "--project-dir",
            str(self.projects[project]),
            "--profiles-dir",
            str(self.profiles),
            "--profile",
            "fusion_tests",
            "--target",
            "redshift",
            "--threads",
            str(a.threads),
            "--target-path",
            str(run_dir / "target"),
            "--log-path",
            str(run_dir / "logs"),
        ]
        if arm != "V1":
            cmd.append("--no-send-anonymous-usage-stats")
        return cmd, self.projects[project]

    def wait_until_quiet(self) -> int:
        """Wait for other statements to finish. Returns how many other sessions remain (noise)."""
        for _ in range(30):
            if (
                self.query(
                    "select count(*) from stv_recents where status = 'Running' and pid <> pg_backend_pid()"
                )[0][0]
                == 0
            ):
                break
            time.sleep(2)
        return self.query("select count(*) from stv_sessions")[0][0] - 1

    def run_once(self, arm: str, block: int, order: int) -> dict[str, Any]:
        other_sessions = self.wait_until_quiet()
        run_dir = Path(tempfile.mkdtemp(prefix="txnrun_", dir=self.work))
        cmd, cwd = self.command(arm, run_dir)
        started, t0 = dt.datetime.now(dt.timezone.utc), time.time()
        proc = subprocess.run(
            cmd, cwd=cwd, capture_output=True, text=True, timeout=1500, check=False
        )
        wall = time.time() - t0
        finished = dt.datetime.now(dt.timezone.utc)
        times, failed = node_times(run_dir / "target")
        time.sleep(3)  # stl_commit_stats lags a little
        fmt = "%Y-%m-%d %H:%M:%S"
        commits, queue_avg, queue_max = self.query(
            "select count(*), coalesce(avg(datediff(ms, startqueue, startwork)), 0), coalesce(max(datediff(ms, startqueue, startwork)), 0) "
            "from stl_commit_stats where startqueue >= %s and startqueue <= %s",
            (started.strftime(fmt), finished.strftime(fmt)),
        )[0]
        record: dict[str, Any] = {
            "threads": self.args.threads,
            "block": block,
            "order": order,
            "arm": arm,
            "rc": proc.returncode,
            "wall": round(wall, 1),
            "nodes_ok": len(times),
            "nodes_err": failed,
            "node_mean": round(st.mean(times), 2) if times else None,
            "node_median": round(st.median(times), 2) if times else None,
            "node_p95": round(percentile(times, 0.95), 2) if times else None,
            "node_max": round(max(times), 2) if times else None,
            "stmts": statement_stats(run_dir / "logs") if arm != "V1" else {},
            "pre_other_sessions": other_sessions,
            "commits": int(commits),
            "commit_queue_avg_ms": round(float(queue_avg)),
            "commit_queue_max_ms": int(queue_max),
        }
        if proc.returncode != 0:
            record["output_tail"] = (proc.stdout + proc.stderr)[-1500:]
        shutil.rmtree(run_dir, ignore_errors=True)
        with Path(self.args.out).open("a") as out:
            out.write(json.dumps(record) + "\n")
        print(
            json.dumps(
                {
                    k: record[k]
                    for k in (
                        "block",
                        "arm",
                        "rc",
                        "wall",
                        "node_mean",
                        "commits",
                        "commit_queue_avg_ms",
                    )
                }
            ),
            flush=True,
        )
        time.sleep(5)
        return record


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--v2-bin", required=True, help="Fusion binary under test")
    parser.add_argument(
        "--pristine-bin", help="Fusion binary without the change (arm P)"
    )
    parser.add_argument(
        "--v1-dbt", help="`dbt` of a Python dbt-redshift environment (arm V1)"
    )
    parser.add_argument(
        "--arms", default="A,B", help="comma-separated, from A,B,E,P,V1 (default: A,B)"
    )
    parser.add_argument("--threads", type=int, default=64)
    parser.add_argument("--blocks", type=int, default=5)
    parser.add_argument("--models", type=int, default=48)
    parser.add_argument(
        "--schema", default="rca_txn_exp", help="scratch schema; dropped at the end"
    )
    parser.add_argument("--out", default="results.jsonl")
    parser.add_argument(
        "--skip-setup", action="store_true", help="the models already exist in --schema"
    )
    args = parser.parse_args()
    arms = args.arms.split(",")
    for arm, needed in (("P", args.pristine_bin), ("V1", args.v1_dbt)):
        if arm in arms and not needed:
            parser.error(
                f"arm {arm} needs --{'pristine-bin' if arm == 'P' else 'v1-dbt'}"
            )

    work = Path(tempfile.mkdtemp(prefix="txnexp_"))
    try:
        projects = gen_project.generate(work, args.models)
        profiles = common.write_scoped_profiles(work / "profiles", args.schema)
        exp = Experiment(args, work, projects, profiles)
        exp.admin.execute(f'create schema if not exists "{args.schema}"')
        if not args.skip_setup:  # untimed: create the tables so every measured run takes the incremental path
            cmd, cwd = exp.command("A", work / "setup")
            rc = subprocess.run(
                cmd, cwd=cwd, capture_output=True, text=True, timeout=1500, check=False
            ).returncode
            print(f"setup run rc = {rc}", flush=True)
        sha = hashlib.sha256(Path(args.v2_bin).read_bytes()).hexdigest()[:16]
        meta = {
            "meta": True,
            "v2_sha": sha,
            "threads": args.threads,
            "arms": arms,
            "models": args.models,
            "started": str(dt.datetime.now(dt.timezone.utc)),
        }
        with Path(args.out).open("a") as out:
            out.write(json.dumps(meta) + "\n")
        rng = random.Random()
        for block in range(args.blocks):
            order = arms[:]
            rng.shuffle(order)
            for i, arm in enumerate(order):
                exp.run_once(arm, block, i)
        exp.admin.execute(f'drop schema if exists "{args.schema}" cascade')
    finally:
        shutil.rmtree(work, ignore_errors=True)  # removes the scoped profiles.yml


if __name__ == "__main__":
    main()
