"""Generate the dbt project variants for the transaction experiment.

Every variant has the same incremental models with `persist_docs` on every column.
Only the setting under test differs:

  A  no setting (Fusion's default: each statement commits on its own)
  B  `redshift_use_transactions: true`
  E  default settings, plus a macro override that wraps only the column comments in one
     transaction (the narrow alternative to B)

usage: gen_project.py OUT_DIR [--models N] [--columns N]
"""

from __future__ import annotations

import argparse
import shutil
from pathlib import Path

import yaml

COMMENTS_ONLY = """\
{# Arm E: one explicit transaction around the persist_docs column comments, nothing else. #}
{% macro redshift__alter_column_comment(relation, column_dict) %}
  {% set statements = postgres__alter_column_comment(relation, column_dict) %}
  {% if statements | trim | length > 0 %}
    begin;
    {{ statements }}
    commit;
  {% endif %}
{% endmacro %}
"""

VARIANTS = {
    "A": {"flags": {}, "macro": None},
    "B": {"flags": {"redshift_use_transactions": True}, "macro": None},
    "E": {"flags": {}, "macro": COMMENTS_ONLY},
}


def generate(out_dir: Path, models: int = 48, columns: int = 31) -> dict[str, Path]:
    """Write one project per variant under `out_dir`. Returns the project directory of each."""
    select = ",\n  ".join(f"'v{i}'::varchar(64) as c{i}" for i in range(columns))
    schema_yml = {
        "version": 2,
        "models": [
            {
                "name": f"rca_txn_{n}",
                "description": f"node {n}",
                "columns": [
                    {"name": f"c{i}", "description": f"doc for c{i}"}
                    for i in range(columns)
                ],
            }
            for n in range(models)
        ],
    }
    projects: dict[str, Path] = {}
    for arm, variant in VARIANTS.items():
        root = out_dir / f"proj_{arm}"
        shutil.rmtree(root, ignore_errors=True)
        (root / "models").mkdir(parents=True)
        (root / "macros").mkdir()
        project = {
            "name": "rca_txn",
            "version": "1.0",
            "profile": "fusion_tests",
            "model-paths": ["models"],
            "macro-paths": ["macros"],
            "models": {
                "rca_txn": {"+persist_docs": {"relation": True, "columns": True}}
            },
        }
        if variant["flags"]:
            project["flags"] = variant["flags"]
        (root / "dbt_project.yml").write_text(yaml.safe_dump(project, sort_keys=False))
        (root / "models" / "schema.yml").write_text(
            yaml.safe_dump(schema_yml, sort_keys=False)
        )
        for n in range(models):
            (root / "models" / f"rca_txn_{n}.sql").write_text(
                f"{{{{ config(materialized='incremental') }}}}\nselect\n  {select}\n"
            )
        if variant["macro"]:
            (root / "macros" / "comments_only.sql").write_text(variant["macro"])
        projects[arm] = root
    return projects


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("out_dir", type=Path)
    parser.add_argument("--models", type=int, default=48)
    parser.add_argument("--columns", type=int, default=31)
    args = parser.parse_args()
    for arm, path in generate(args.out_dir, args.models, args.columns).items():
        print(f"{arm}: {path}")
