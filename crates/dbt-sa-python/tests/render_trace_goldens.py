"""Normalize semantic fixtures; regenerate them through the real exporter.

With this branch's dbt-core installed, run with DuckDB available:
uv run --no-project --active --with duckdb python tests/render_trace_goldens.py
"""

import json
import os
import shutil
import tempfile
from pathlib import Path


def normalize_trace(trace):
    trace = dict(trace)
    trace["dbt_version"] = "0.0.0-example"
    trace["invocation_id"] = "00000000-0000-4000-8000-000000000001"
    trace["source_path"] = trace["filename"]
    trace.pop("ingested_at", None)
    return trace


def regenerate():
    import duckdb
    from dbt.cli.main import dbtRunner

    fixture = Path(__file__).parent / "integration/fixtures/render_trace"
    previous = Path.cwd()
    with tempfile.TemporaryDirectory(prefix="render-trace-goldens-") as temporary:
        project = Path(temporary) / "project"
        shutil.copytree(fixture, project)
        (project / "packages.yml").write_text((project / "star_packages.yml").read_text())
        runner = dbtRunner()
        try:
            os.chdir(project)
            for command in [
                ["deps"],
                [
                    "compile",
                    "--select",
                    "common_patterns",
                    "star_usage",
                    "--feature",
                    "write_render_trace",
                ],
            ]:
                result = runner.invoke([*command, "--profiles-dir", str(project)])
                if not result.success:
                    raise RuntimeError(f"Exporter command failed: {command}: {result.exception}")
            with duckdb.connect() as connection:
                rows = connection.execute(
                    "select to_json(t) from read_parquet(?) t "
                    "qualify row_number() over (partition by unique_id "
                    "order by ingested_at desc) = 1",
                    [str(project / "target/private/metadata/compile/render_traces/v1_*.parquet")],
                ).fetchall()
            traces = [json.loads(row[0]) for row in rows]
            golden = {
                trace["unique_id"]: normalize_trace(trace)
                for trace in traces
                if trace["unique_id"]
                in {
                    "model.render_trace.common_patterns",
                    "model.render_trace.star_usage",
                }
            }
            if len(golden) != 2:
                raise RuntimeError(f"Expected both semantic fixture models, got {list(golden)}")
        finally:
            os.chdir(previous)
    path = fixture / "goldens/render_traces.json"
    path.write_text(json.dumps(golden, indent=2, ensure_ascii=False, sort_keys=True) + "\n")


if __name__ == "__main__":
    regenerate()
