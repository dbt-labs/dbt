#!/usr/bin/env python3
"""Convert MetricFlow table snapshot YAML files into DuckDB-compatible SQL.

Reads YAML files from ~/projects/metricflow/tests_metricflow/fixtures/source_table_snapshots/
and generates CREATE TABLE + INSERT INTO SQL files for DuckDB.
"""

import os
import re
import sys
from pathlib import Path

import yaml

SNAPSHOTS_DIR = Path.home() / "projects/metricflow/tests_metricflow/fixtures/source_table_snapshots"
OUTPUT_DIR = Path(__file__).parent

BATCH_SIZE = 50

# Tables already in the existing Rust test (exclude from simple_model_extra)
EXISTING_SIMPLE_MODEL_TABLES = {
    "dim_listings.yaml",
    "dim_lux_listings.yaml",
    "dim_primary_accounts.yaml",
    "fct_bookings_dt.yaml",
}


def yaml_type_to_duckdb(col_type: str, values: list[str | None]) -> str:
    """Map MetricFlow YAML types to DuckDB types.

    For TIME columns, inspect actual data values to decide DATE vs TIMESTAMP.
    """
    col_type = col_type.upper()
    if col_type == "STRING":
        return "VARCHAR"
    elif col_type == "INT":
        return "INTEGER"
    elif col_type == "FLOAT":
        return "DOUBLE"
    elif col_type == "BOOLEAN":
        return "BOOLEAN"
    elif col_type == "TIME":
        # Inspect values: if any contain a space (datetime), use TIMESTAMP
        for v in values:
            if v is not None and " " in str(v):
                return "TIMESTAMP"
        return "DATE"
    else:
        return col_type


def render_value(val, col_type: str, duckdb_type: str) -> str:
    """Render a single cell value as a DuckDB SQL literal."""
    if val is None:
        return "NULL"

    s = str(val)

    if col_type.upper() == "BOOLEAN":
        if s == "True":
            return "true"
        elif s == "False":
            return "false"
        else:
            return s.lower()

    if col_type.upper() == "TIME":
        # Dates and timestamps: wrap in single quotes
        escaped = s.replace("'", "''")
        return f"'{escaped}'"

    if col_type.upper() == "FLOAT":
        return s

    if col_type.upper() == "INT":
        return s

    # STRING: wrap in single quotes
    escaped = s.replace("'", "''")
    return f"'{escaped}'"


def parse_yaml_file(filepath: Path) -> dict:
    """Parse a MetricFlow table snapshot YAML file."""
    with open(filepath) as f:
        data = yaml.safe_load(f)
    return data["table_snapshot"]


def generate_sql_for_table(snapshot: dict) -> str:
    """Generate CREATE TABLE + INSERT INTO SQL for one table snapshot."""
    table_name = snapshot["table_name"]
    col_defs = snapshot["column_definitions"]
    rows = snapshot.get("rows", [])

    # Collect sample values per column to determine TIME -> DATE vs TIMESTAMP
    col_samples: dict[int, list] = {i: [] for i in range(len(col_defs))}
    for row in rows:
        for i, val in enumerate(row):
            if val is not None:
                col_samples[i].append(str(val))

    # Determine DuckDB types
    duckdb_types = []
    for i, col in enumerate(col_defs):
        duckdb_types.append(yaml_type_to_duckdb(col["type"], col_samples.get(i, [])))

    # CREATE TABLE
    col_parts = []
    for col, dt in zip(col_defs, duckdb_types):
        col_parts.append(f"    {col['name']} {dt}")
    create_sql = f"CREATE TABLE {table_name} (\n" + ",\n".join(col_parts) + "\n);\n"

    if not rows:
        return create_sql

    # INSERT INTO in batches
    insert_parts = []
    for batch_start in range(0, len(rows), BATCH_SIZE):
        batch = rows[batch_start : batch_start + BATCH_SIZE]
        value_rows = []
        for row in batch:
            vals = []
            for i, val in enumerate(row):
                vals.append(render_value(val, col_defs[i]["type"], duckdb_types[i]))
            value_rows.append(f"    ({', '.join(vals)})")
        insert_sql = f"INSERT INTO {table_name} VALUES\n" + ",\n".join(value_rows) + ";\n"
        insert_parts.append(insert_sql)

    return create_sql + "\n" + "\n".join(insert_parts)


def process_directory(
    subdir: str,
    exclude_files: set[str] | None = None,
    include_files: list[str] | None = None,
) -> list[tuple[str, int]]:
    """Process all YAML files in a subdirectory. Returns list of (table_name, row_count)."""
    dir_path = SNAPSHOTS_DIR / subdir
    if not dir_path.exists():
        print(f"Warning: directory {dir_path} does not exist", file=sys.stderr)
        return []

    if include_files is not None:
        yaml_files = [dir_path / f for f in include_files if (dir_path / f).exists()]
    else:
        yaml_files = sorted(dir_path.glob("*.yaml"))

    if exclude_files:
        yaml_files = [f for f in yaml_files if f.name not in exclude_files]

    results = []
    sql_parts = []
    for filepath in yaml_files:
        snapshot = parse_yaml_file(filepath)
        sql = generate_sql_for_table(snapshot)
        sql_parts.append(sql)
        row_count = len(snapshot.get("rows", []))
        results.append((snapshot["table_name"], row_count))

    return results, "\n".join(sql_parts)


def main():
    summary = []

    # 1. tables_simple_model_extra.sql
    print("Processing simple_model (extra tables)...")
    tables, sql = process_directory("simple_model", exclude_files=EXISTING_SIMPLE_MODEL_TABLES)
    output_path = OUTPUT_DIR / "tables_simple_model_extra.sql"
    output_path.write_text(sql + "\n")
    summary.append(("tables_simple_model_extra.sql", tables))

    # 2. tables_time_spine.sql — daily spine + hour + second + all small sub-second spines
    print("Processing time_spine_table...")
    time_spine_files = [
        "mf_time_spine.yaml",
        "mf_time_spine_hour.yaml",
        "mf_time_spine_second.yaml",
        "mf_time_spine_minute.yaml",
        "mf_time_spine_millisecond.yaml",
        "mf_time_spine_microsecond.yaml",
        "mf_time_spine_nanosecond.yaml",
    ]
    tables, sql = process_directory("time_spine_table", include_files=time_spine_files)
    output_path = OUTPUT_DIR / "tables_time_spine.sql"
    output_path.write_text(sql + "\n")
    summary.append(("tables_time_spine.sql", tables))

    # 3. tables_extended_date_model.sql
    print("Processing extended_date_model...")
    tables, sql = process_directory("extended_date_model")
    output_path = OUTPUT_DIR / "tables_extended_date_model.sql"
    output_path.write_text(sql + "\n")
    summary.append(("tables_extended_date_model.sql", tables))

    # 4. tables_multi_hop_model.sql
    print("Processing multi_hop_join_model...")
    tables, sql = process_directory("multi_hop_join_model")
    output_path = OUTPUT_DIR / "tables_multi_hop_model.sql"
    output_path.write_text(sql + "\n")
    summary.append(("tables_multi_hop_model.sql", tables))

    # Print summary
    print("\n=== Summary ===")
    total_tables = 0
    total_rows = 0
    for filename, tables in summary:
        print(f"\n{filename}:")
        file_rows = 0
        for table_name, row_count in tables:
            print(f"  {table_name}: {row_count} rows")
            file_rows += row_count
        print(f"  Total: {len(tables)} tables, {file_rows} rows")
        total_tables += len(tables)
        total_rows += file_rows
    print(f"\nGrand total: {total_tables} tables, {total_rows} rows")


if __name__ == "__main__":
    main()
