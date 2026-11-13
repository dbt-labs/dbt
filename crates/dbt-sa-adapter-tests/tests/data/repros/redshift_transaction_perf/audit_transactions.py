"""Audit `redshift_use_transactions`: does every transaction end, and is the work committed?

Runs `build`, `snapshot` and `build --full-refresh` on a project that touches the paths that
open transactions: a seed, a view, a table, an incremental model, a snapshot, data tests, a model
in a custom schema (schema creation), and on-run-start and on-run-end hooks.

For each command it reads Fusion's query_log.sql and checks that `begin` equals `commit` plus
`rollback`. It then checks from a separate session that every expected object exists, which fails
if work was left in an uncommitted transaction. Run it with the setting off as a control: that run
must show no begin, commit or rollback at all.

usage: audit_transactions.py --bin PATH [--arm on|off|both]
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

import common
import yaml

SCHEMA, EXTRA_SCHEMA = "rca_audit", "rca_audit_extra"
EXPECTED = [
    (SCHEMA, name)
    for name in (
        "audit_seed",
        "m_view",
        "m_table",
        "m_inc",
        "rca_audit_snap",
        "rca_audit_hook_start",
        "rca_audit_hook_end",
    )
] + [(EXTRA_SCHEMA, "m_other_schema")]


def write_project(root: Path, transactions: bool) -> None:
    for sub in ("models", "seeds", "snapshots"):
        (root / sub).mkdir(parents=True)
    project = {
        "name": "rca_audit",
        "version": "1.0",
        "profile": "fusion_tests",
        "model-paths": ["models"],
        "seed-paths": ["seeds"],
        "snapshot-paths": ["snapshots"],
        "models": {
            "rca_audit": {
                "+persist_docs": {"relation": True, "columns": True},
                "m_other_schema": {"+schema": "extra"},
            }
        },
        "on-run-start": [
            "drop table if exists {{ target.schema }}.rca_audit_hook_start",
            "create table {{ target.schema }}.rca_audit_hook_start as select 1 as x",
        ],
        "on-run-end": [
            "drop table if exists {{ target.schema }}.rca_audit_hook_end",
            "create table {{ target.schema }}.rca_audit_hook_end as select 1 as x",
        ],
    }
    if transactions:
        project["flags"] = {"redshift_use_transactions": True}
    (root / "dbt_project.yml").write_text(yaml.safe_dump(project, sort_keys=False))
    (root / "seeds" / "audit_seed.csv").write_text("id,name\n1,a\n2,b\n")
    (root / "models" / "m_view.sql").write_text(
        "{{ config(materialized='view') }}\nselect * from {{ ref('audit_seed') }}\n"
    )
    (root / "models" / "m_table.sql").write_text(
        "{{ config(materialized='table') }}\nselect * from {{ ref('audit_seed') }}\n"
    )
    (root / "models" / "m_inc.sql").write_text(
        "{{ config(materialized='incremental') }}\nselect * from {{ ref('audit_seed') }}\n"
    )
    (root / "models" / "m_other_schema.sql").write_text(
        "{{ config(materialized='table') }}\nselect 1 as id\n"
    )
    (root / "models" / "schema.yml").write_text(
        "version: 2\nmodels:\n"
        "  - name: m_table\n    description: t\n    columns:\n      - name: id\n        description: i\n        tests: [not_null, unique]\n"
        "  - name: m_inc\n    description: i\n    columns:\n      - name: id\n        description: i\n"
    )
    (root / "snapshots" / "snap.sql").write_text(
        "{% snapshot rca_audit_snap %}{{ config(target_schema=target.schema, unique_key='id', strategy='check', check_cols=['name']) }}\n"
        "select * from {{ ref('audit_seed') }}\n{% endsnapshot %}\n"
    )


def statements(log_path: Path) -> list[str]:
    path = log_path / "query_log.sql"
    if not path.exists():
        return []
    out = []
    for block in path.read_text().split("\n-- created_at:")[1:]:
        if "-- desc:" in block:
            sql = re.sub(
                r"/\*.*?\*/",
                "",
                block.split("-- desc:")[1].split("\n", 1)[1],
                flags=re.DOTALL,
            )
            out.append(sql.strip().rstrip(";").lower())
    return out


def audit(binary: str, transactions: bool) -> bool:
    arm = "on" if transactions else "off"
    work = Path(tempfile.mkdtemp(prefix="txnaudit_"))
    admin = common.connect(common.load_output()).cursor()
    healthy = True
    try:
        project = work / "project"
        write_project(project, transactions)
        profiles = common.write_scoped_profiles(work / "profiles", SCHEMA)
        for schema in (SCHEMA, EXTRA_SCHEMA):
            admin.execute(f'drop schema if exists "{schema}" cascade')
        for tag, command in (
            ("build", ["build"]),
            ("snapshot", ["snapshot"]),
            ("refresh", ["build", "--full-refresh"]),
        ):
            logs = work / f"logs_{tag}"
            proc = subprocess.run(
                [
                    binary,
                    *command,
                    "--project-dir",
                    str(project),
                    "--profiles-dir",
                    str(profiles),
                    "--profile",
                    "fusion_tests",
                    "--target",
                    "redshift",
                    "--threads",
                    "4",
                    "--target-path",
                    str(work / f"target_{tag}"),
                    "--log-path",
                    str(logs),
                    "--no-send-anonymous-usage-stats",
                ],
                capture_output=True,
                text=True,
                timeout=900,
                check=False,
            )
            sql = statements(logs)
            begins, commits, rollbacks = (
                sql.count("begin"),
                sql.count("commit"),
                sql.count("rollback"),
            )
            summary = next(
                (
                    line
                    for line in (proc.stdout + proc.stderr).splitlines()
                    if line.startswith("Summary")
                ),
                "no summary",
            )
            paired = begins == commits + rollbacks
            control_clean = transactions or (begins + commits + rollbacks) == 0
            healthy &= proc.returncode == 0 and paired and control_clean
            print(
                f"[{arm}] {' '.join(command):22} rc={proc.returncode} {summary}  begin={begins} commit={commits} "
                f"rollback={rollbacks}  {'paired' if paired else 'UNPAIRED'}",
                flush=True,
            )
        missing = []
        for schema, name in EXPECTED:
            admin.execute(
                "select count(*) from pg_class c join pg_namespace n on n.oid = c.relnamespace where n.nspname = %s and c.relname = %s",
                (schema, name),
            )
            if admin.fetchone()[0] != 1:
                missing.append(f"{schema}.{name}")
        healthy &= not missing
        print(
            f"[{arm}] visible from another session: {len(EXPECTED) - len(missing)}/{len(EXPECTED)}; missing={missing}"
        )
    finally:
        for schema in (SCHEMA, EXTRA_SCHEMA):
            admin.execute(f'drop schema if exists "{schema}" cascade')
        shutil.rmtree(work, ignore_errors=True)  # removes the scoped profiles.yml
    return healthy


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--bin", required=True, help="Fusion binary under test")
    parser.add_argument("--arm", choices=("on", "off", "both"), default="both")
    args = parser.parse_args()
    arms = [False, True] if args.arm == "both" else [args.arm == "on"]
    results = [audit(args.bin, transactions) for transactions in arms]
    print("AUDIT PASSED" if all(results) else "AUDIT FAILED")
    raise SystemExit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
