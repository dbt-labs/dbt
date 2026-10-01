#!/usr/bin/env python3
"""Convert MetricFlow integration test YAML files into a JSON file consumable by a Rust test harness.

Usage:
    python convert_python_tests.py

Reads YAML files from ~/projects/metricflow/tests_metricflow/integration/test_cases/itest_*.yaml
and writes JSON to the same directory as this script: ported_test_cases.json
"""

import json
import os
import re
import sys
from datetime import datetime, timedelta
from pathlib import Path

import yaml

# ── Paths ────────────────────────────────────────────────────────────────────

INPUT_DIR = Path.home() / "projects" / "metricflow" / "tests_metricflow" / "integration" / "test_cases"
OUTPUT_PATH = Path(__file__).parent / "ported_test_cases.json"

# ── Known names for classification ───────────────────────────────────────────

TIME_DIMENSION_NAMES = {
    "metric_time", "ds", "ds_partitioned", "paid_at", "created_at",
    "archived_at", "last_profile_edit_ts", "last_login_ts", "bio_added_ts",
    "window_start", "window_end", "account_month", "verification_ts",
    "dt",
    # extended date model names
    "listing_creation_ds",
    # monthly time dimension
    "ds_month",
}

ENTITY_NAMES = {
    "listing", "host", "guest", "user", "lux_listing", "booking", "company",
    "verification", "view", "visit", "account", "customer", "bridge_account",
    "third_hop",
}

# Time granularity names (standard MetricFlow grains)
STANDARD_GRAINS = {
    "day", "week", "month", "quarter", "year",
    "second", "minute", "hour", "millisecond",
}

ALREADY_PORTED = {
    "count_distinct_with_constraint",
    "cumulative_metric_in_where_filter",
    "derived_fill_nulls_for_one_input_metric",
    "derived_metric_in_where_filter",
    "derived_metrics_with_null_dimension_values",
    "identifier_constrained_metric",
    "metric_with_metric_in_where_filter",
    "multiple_cumulative_metrics",
    "multiple_metrics_in_where_filter",
    "offset_to_grain_with_agg_time_dim",
    "offset_window_with_agg_time_dim",
    "query_with_3_metrics",
    "ratio_metric_in_where_filter",
    "ratio_with_non_ratio",
    "ratio_with_zero_denominator",
}

# DatePart mapping for DuckDB
DATE_PART_MAP = {
    "DOW": "isodow",
    "DAY": "day",
    "MONTH": "month",
    "QUARTER": "quarter",
    "YEAR": "year",
    "DOY": "doy",
}

# ── Jinja rendering helpers (DuckDB dialect) ─────────────────────────────────


def _add_one(date_str: str) -> str:
    """Add one day (for date-length strings) or one second (for timestamp-length strings)."""
    if len(date_str) <= 10:  # YYYY-MM-DD
        d = datetime.strptime(date_str, "%Y-%m-%d")
        return (d + timedelta(days=1)).strftime("%Y-%m-%d")
    else:  # timestamp
        d = datetime.strptime(date_str, "%Y-%m-%d %H:%M:%S")
        return (d + timedelta(seconds=1)).strftime("%Y-%m-%d %H:%M:%S")


def render_time_constraint(ds_expr: str, start_time: str = None, stop_time: str = None) -> str:
    """Render a time constraint for DuckDB."""
    parts = []
    if start_time:
        parts.append(f"CAST({ds_expr} AS TIMESTAMP) >= CAST('{start_time}' AS TIMESTAMP)")
    if stop_time:
        stop_plus = _add_one(stop_time)
        parts.append(f"CAST({ds_expr} AS TIMESTAMP) < CAST('{stop_plus}' AS TIMESTAMP)")
    return " AND ".join(parts)


def render_date_trunc(col: str, grain: str) -> str:
    return f"DATE_TRUNC('{grain.lower()}', CAST({col} AS TIMESTAMP))"


def render_extract(col: str, date_part: str) -> str:
    duckdb_part = DATE_PART_MAP.get(date_part.upper(), date_part.lower())
    return f"EXTRACT({duckdb_part} FROM CAST({col} AS TIMESTAMP))"


def render_date_sub(alias: str, col: str, count: int, grain: str) -> str:
    g = grain.lower()
    if g == "quarter":
        count = count * 3
        g = "month"
    elif g == "year":
        count = count * 12
        g = "month"
    return f"{alias}.{col} - INTERVAL {count} {g}"


def render_date_add(date_expr: str, count_expr: str, grain: str) -> str:
    g = grain.lower()
    if g == "quarter":
        # multiply count by 3
        count_expr = f"({count_expr}) * 3"
        g = "month"
    elif g == "year":
        count_expr = f"({count_expr}) * 12"
        g = "month"
    return f"{date_expr} + INTERVAL ({count_expr}) {g}"


def render_percentile_expr(col: str, percentile: float, use_discrete: bool, use_approximate: bool) -> str:
    if use_approximate:
        return f"APPROX_QUANTILE({col}, {percentile})"
    elif use_discrete:
        return f"PERCENTILE_DISC({percentile}) WITHIN GROUP (ORDER BY ({col}))"
    else:
        return f"PERCENTILE_CONT({percentile}) WITHIN GROUP (ORDER BY ({col}))"


def render_dimension_template(name: str, entity_path: list = None) -> str:
    if entity_path:
        return "{{ " + f"Dimension('{name}', entity_path={entity_path})" + " }}"
    return "{{ " + f"Dimension('{name}')" + " }}"


def render_entity_template(name: str) -> str:
    return "{{ " + f"Entity('{name}')" + " }}"


def render_metric_template(name: str, entity_path: list = None) -> str:
    if entity_path:
        return "{{ " + f"Metric('{name}', {entity_path})" + " }}"
    return "{{ " + f"Metric('{name}')" + " }}"


def render_time_dimension_template(name: str, grain: str = None) -> str:
    if grain:
        return "{{ " + f"TimeDimension('{name}', '{grain}')" + " }}"
    return "{{ " + f"TimeDimension('{name}')" + " }}"


# ── Main Jinja renderer ─────────────────────────────────────────────────────


def render_jinja(text: str) -> str:
    """Render Jinja templates in check_query / where_filter strings.

    This is a regex-based approach rather than using a full Jinja engine,
    because the templates use Python-like expressions (e.g. TimeGranularity.DAY)
    that are not valid Jinja variables.
    """
    if text is None:
        return None

    result = text

    # Simple substitutions
    result = result.replace("{{ source_schema }}", "main")
    result = result.replace("{{source_schema}}", "main")
    result = result.replace("{{ mf_time_spine_source }}", "main.mf_time_spine")

    # {{ double_data_type_name }}
    result = result.replace("{{ double_data_type_name }}", "DOUBLE")

    # {{ generate_random_uuid() }}
    result = result.replace("{{ generate_random_uuid() }}", "GEN_RANDOM_UUID()")

    # {{ cast_to_ts('...') }}
    result = re.sub(
        r"""\{\{\s*cast_to_ts\(\s*['"](.*?)['"]\s*\)\s*\}\}""",
        lambda m: f"CAST('{m.group(1)}' AS TIMESTAMP)",
        result,
    )

    # render_time_constraint with named kwargs (start_time=, stop_time= / end_time=) —
    # handles cases like render_time_constraint("col", start_time="2020-01-01")
    # Also handles when first arg is a nested call like render_date_trunc(...)
    def _resolve_time_constraint(m):
        full = m.group(0)
        # Parse the arguments more carefully
        inner = full[full.index("render_time_constraint(") + len("render_time_constraint("):]
        # Find matching closing paren
        depth = 1
        i = 0
        while i < len(inner) and depth > 0:
            if inner[i] == '(':
                depth += 1
            elif inner[i] == ')':
                depth -= 1
            i += 1
        args_str = inner[:i - 1]

        # Parse arguments - split by comma but respect parentheses and quotes
        args = _split_args(args_str)

        # First arg is the column expression
        col_arg = args[0].strip().strip("'\"")

        # Check if col_arg is a nested render call
        if "render_date_trunc" in args[0] or "render_time_dimension_template" in args[0]:
            col_arg = _eval_nested(args[0].strip())

        start = None
        stop = None

        for i_arg, arg in enumerate(args[1:], 1):
            arg = arg.strip()
            if arg.startswith("start_time="):
                start = arg.split("=", 1)[1].strip().strip("'\"")
            elif arg.startswith("stop_time="):
                stop = arg.split("=", 1)[1].strip().strip("'\"")
            elif arg.startswith("end_time="):
                stop = arg.split("=", 1)[1].strip().strip("'\"")
            else:
                # positional
                val = arg.strip("'\"")
                if i_arg == 1:
                    start = val
                elif i_arg == 2:
                    stop = val

        return render_time_constraint(col_arg, start, stop)

    # First, handle render_time_constraint where first arg is render_date_trunc or render_time_dimension_template
    result = re.sub(
        r"""\{\{\s*render_time_constraint\(.*?\)\s*\}\}""",
        _resolve_time_constraint,
        result,
    )
    # There might be more complex multi-line cases; handle them too
    # Actually the regex above handles most. Let's also catch multi-line ones.

    # render_date_trunc
    def _replace_date_trunc(m):
        col = m.group(1).strip().strip("'\"")
        grain = m.group(2).strip()
        return render_date_trunc(col, grain)

    result = re.sub(
        r"""\{\{\s*render_date_trunc\(\s*["'](.*?)["']\s*,\s*TimeGranularity\.(\w+)\s*\)\s*\}\}""",
        _replace_date_trunc,
        result,
    )

    # render_extract
    def _replace_extract(m):
        col = m.group(1).strip().strip("'\"")
        part = m.group(2).strip()
        return render_extract(col, part)

    result = re.sub(
        r"""\{\{\s*render_extract\(\s*["'](.*?)["']\s*,\s*DatePart\.(\w+)\s*\)\s*\}\}""",
        _replace_extract,
        result,
    )

    # render_date_sub
    def _replace_date_sub(m):
        alias = m.group(1).strip().strip("'\"")
        col = m.group(2).strip().strip("'\"")
        count = int(m.group(3).strip())
        grain = m.group(4).strip()
        return render_date_sub(alias, col, count, grain)

    result = re.sub(
        r"""\{\{\s*render_date_sub\(\s*["'](.*?)["']\s*,\s*["'](.*?)["']\s*,\s*(\d+)\s*,\s*TimeGranularity\.(\w+)\s*\)\s*\}\}""",
        _replace_date_sub,
        result,
    )

    # render_date_add
    def _replace_date_add(m):
        date_expr = m.group(1).strip().strip("'\"")
        count_expr = m.group(2).strip().strip("'\"")
        grain = m.group(3).strip()
        return render_date_add(date_expr, count_expr, grain)

    result = re.sub(
        r"""\{\{\s*render_date_add\(\s*["'](.*?)["']\s*,\s*["'](.*?)["']\s*,\s*TimeGranularity\.(\w+)\s*\)\s*\}\}""",
        _replace_date_add,
        result,
    )

    # render_percentile_expr
    def _replace_percentile(m):
        col = m.group(1).strip().strip("'\"")
        pct = float(m.group(2).strip())
        discrete = m.group(3).strip()
        approx = m.group(4).strip()
        use_discrete = discrete in ("True", "true", "1")
        use_approx = approx in ("True", "true", "1")
        return render_percentile_expr(col, pct, use_discrete, use_approx)

    result = re.sub(
        r"""\{\{\s*render_percentile_expr\(\s*["'](.*?)["']\s*,\s*([\d.]+)\s*,\s*(\w+)\s*,\s*(\w+)\s*\)\s*\}\}""",
        _replace_percentile,
        result,
    )

    # render_dimension_template with entity_path
    def _replace_dim_template_with_path(m):
        name = m.group(1)
        path_str = m.group(2)
        # Parse the list
        items = re.findall(r"'([^']*)'", path_str)
        return render_dimension_template(name, items)

    result = re.sub(
        r"""\{\{\s*render_dimension_template\(\s*['"](.*?)['"]\s*,\s*entity_path\s*=\s*(\[.*?\])\s*\)\s*\}\}""",
        _replace_dim_template_with_path,
        result,
    )

    # render_dimension_template (simple)
    result = re.sub(
        r"""\{\{\s*render_dimension_template\(\s*['"](.*?)['"]\s*\)\s*\}\}""",
        lambda m: render_dimension_template(m.group(1)),
        result,
    )

    # render_entity_template
    result = re.sub(
        r"""\{\{\s*render_entity_template\(\s*['"](.*?)['"]\s*\)\s*\}\}""",
        lambda m: render_entity_template(m.group(1)),
        result,
    )

    # render_metric_template with entity path
    def _replace_metric_template(m):
        name = m.group(1)
        path_str = m.group(2)
        items = re.findall(r"'([^']*)'", path_str)
        return render_metric_template(name, items)

    result = re.sub(
        r"""\{\{\s*render_metric_template\(\s*['"](.*?)['"]\s*,\s*(\[.*?\])\s*\)\s*\}\}""",
        _replace_metric_template,
        result,
    )

    # render_metric_template (simple, no path)
    result = re.sub(
        r"""\{\{\s*render_metric_template\(\s*['"](.*?)['"]\s*\)\s*\}\}""",
        lambda m: render_metric_template(m.group(1)),
        result,
    )

    # render_time_dimension_template with grain
    result = re.sub(
        r"""\{\{\s*render_time_dimension_template\(\s*['"](.*?)['"]\s*,\s*['"](.*?)['"]\s*\)\s*\}\}""",
        lambda m: render_time_dimension_template(m.group(1), m.group(2)),
        result,
    )

    # render_time_dimension_template without grain
    result = re.sub(
        r"""\{\{\s*render_time_dimension_template\(\s*['"](.*?)['"]\s*\)\s*\}\}""",
        lambda m: render_time_dimension_template(m.group(1)),
        result,
    )

    return result


def _split_args(s: str) -> list:
    """Split a string of function arguments by commas, respecting nested parens and quotes."""
    args = []
    depth = 0
    in_quote = None
    current = []
    for ch in s:
        if in_quote:
            current.append(ch)
            if ch == in_quote:
                in_quote = None
        elif ch in ('"', "'"):
            in_quote = ch
            current.append(ch)
        elif ch == '(':
            depth += 1
            current.append(ch)
        elif ch == ')':
            depth -= 1
            current.append(ch)
        elif ch == ',' and depth == 0:
            args.append(''.join(current))
            current = []
        else:
            current.append(ch)
    if current:
        args.append(''.join(current))
    return args


def _eval_nested(expr: str) -> str:
    """Evaluate a nested render call that appears as an argument to render_time_constraint."""
    expr = expr.strip()

    # render_date_trunc("col", TimeGranularity.GRAIN)
    m = re.match(r"""render_date_trunc\(\s*["'](.*?)["']\s*,\s*TimeGranularity\.(\w+)\s*\)""", expr)
    if m:
        return render_date_trunc(m.group(1), m.group(2))

    # render_time_dimension_template('name', 'grain')
    m = re.match(r"""render_time_dimension_template\(\s*['"](.*?)['"]\s*,\s*['"](.*?)['"]\s*\)""", expr)
    if m:
        return render_time_dimension_template(m.group(1), m.group(2))

    # render_time_dimension_template('name')
    m = re.match(r"""render_time_dimension_template\(\s*['"](.*?)['"]\s*\)""", expr)
    if m:
        return render_time_dimension_template(m.group(1))

    return expr


# ── Group-by classification ──────────────────────────────────────────────────


def _is_time_dimension_name(name: str) -> bool:
    """Check if the base name (without entity prefix and grain suffix) is a time dimension."""
    parts = name.split("__")

    # Check if the full name or the last component (after entity prefix) matches
    # a known time dimension name
    for i in range(len(parts)):
        candidate = "__".join(parts[i:])
        # Strip any trailing grain suffix
        for grain in STANDARD_GRAINS:
            if candidate.endswith(f"__{grain}"):
                base = candidate[: -(len(grain) + 2)]
                if base in TIME_DIMENSION_NAMES:
                    return True
        if candidate in TIME_DIMENSION_NAMES:
            return True

    return False


def _extract_time_dim_parts(name: str):
    """Given a dunder-separated group_by string, extract (dim_name, grain) if it's a time dimension.

    Returns (dim_name, grain) or None if not a time dimension.
    """
    parts = name.split("__")

    # Check if the last part is a standard grain
    last = parts[-1]
    if last in STANDARD_GRAINS and len(parts) >= 2:
        base = "__".join(parts[:-1])
        if _is_time_dimension_name(base):
            return (base, last)

    # Check if it's a custom granularity (e.g., alien_day) - look through all possible split points
    # For something like "booking__ds__alien_day", check if "booking__ds" is a time dim
    for i in range(len(parts) - 1, 0, -1):
        candidate_base = "__".join(parts[:i])
        candidate_grain = "__".join(parts[i:])
        if _is_time_dimension_name(candidate_base) and candidate_grain not in ENTITY_NAMES:
            # Only treat as custom grain if the base is actually a time dimension
            # and the suffix isn't just another entity
            # Check: is the base actually recognized?
            if _name_matches_time_dim(candidate_base):
                return (candidate_base, candidate_grain)

    # No grain suffix; check if the whole name is a time dimension
    if _is_time_dimension_name(name):
        return (name, "day")

    return None


def _name_matches_time_dim(name: str) -> bool:
    """Check if a name (possibly entity-prefixed) is a recognized time dimension."""
    if name in TIME_DIMENSION_NAMES:
        return True
    parts = name.split("__")
    for i in range(1, len(parts)):
        tail = "__".join(parts[i:])
        if tail in TIME_DIMENSION_NAMES:
            return True
    return False


def classify_group_by(name: str) -> str:
    """Convert a dunder-separated group_by string to the MetricFlow format.

    Returns something like:
        TimeDimension('metric_time', 'day')
        Dimension('booking__is_instant')
        Entity('listing')
    """
    # Check entities first (exact match only)
    if name in ENTITY_NAMES:
        return f"Entity('{name}')"

    # Check time dimension
    td = _extract_time_dim_parts(name)
    if td:
        dim_name, grain = td
        return f"TimeDimension('{dim_name}', '{grain}')"

    # Everything else is a categorical dimension
    return f"Dimension('{name}')"


def classify_group_by_obj(obj: dict) -> str:
    """Convert a group_by_obj (with name, optional grain, optional date_part) to MetricFlow format."""
    name = obj["name"]
    grain = obj.get("grain")
    date_part = obj.get("date_part")

    # Determine if it's a time dimension
    is_td = False

    if date_part:
        is_td = True
    elif grain:
        is_td = True
    else:
        # Check if the name itself is a time dimension
        td = _extract_time_dim_parts(name)
        if td:
            is_td = True

    if is_td:
        if not grain:
            # Extract grain from the name or default to 'day'
            td = _extract_time_dim_parts(name)
            if td:
                name, grain = td
            else:
                grain = "day"

        if date_part:
            return f"TimeDimension('{name}', '{grain}', date_part='{date_part}')"
        else:
            return f"TimeDimension('{name}', '{grain}')"

    # Entity check
    if name in ENTITY_NAMES:
        return f"Entity('{name}')"

    # Categorical dimension
    return f"Dimension('{name}')"


# ── Skip reason logic ────────────────────────────────────────────────────────


def determine_skip_reason(test: dict) -> str | None:
    model = test.get("model", "")
    features = test.get("required_features", [])
    saved_query = test.get("saved_query_name")
    min_max = test.get("min_max_only", False)
    apply_group_by = test.get("apply_group_by")

    if model == "SCD_MODEL":
        return "SCD model not yet supported"

    for feat in features:
        if feat.startswith("APPROXIMATE_"):
            return f"requires {feat}"

    # Check for alien_day custom granularity usage
    group_bys = test.get("group_bys", [])
    group_by_objs = test.get("group_by_objs", [])
    where_filter = test.get("where_filter", "")

    check_query = test.get("check_query", "")
    metrics = test.get("metrics", [])

    has_alien_day = False
    for gb in group_bys:
        if "alien_day" in gb:
            has_alien_day = True
    for gbo in group_by_objs:
        if gbo.get("grain") == "alien_day" or "alien_day" in gbo.get("name", ""):
            has_alien_day = True
    if where_filter and "alien_day" in str(where_filter):
        has_alien_day = True
    # Also check metrics and check_query for alien_day references
    for m in metrics:
        if "alien_day" in m:
            has_alien_day = True
    if check_query and "alien_day" in str(check_query):
        has_alien_day = True

    if has_alien_day:
        return "custom granularity not yet supported"

    if saved_query:
        return "saved query test"

    if min_max:
        return "min_max_only not supported"

    if apply_group_by is False:
        return "apply_group_by=false not supported"

    return None


# ── Main conversion ─────────────────────────────────────────────────────────


def convert_test(test: dict, filename: str) -> dict:
    name = test.get("name", "").rstrip(".")  # some names have trailing period
    model = test.get("model", "SIMPLE_MODEL")
    metrics = test.get("metrics", [])
    group_bys = test.get("group_bys", [])
    group_by_objs = test.get("group_by_objs", [])
    where_filter = test.get("where_filter")
    time_constraint = test.get("time_constraint")
    order_bys = test.get("order_bys")
    limit = test.get("limit")
    check_query_raw = test.get("check_query", "")
    check_order = test.get("check_order", False)
    allow_empty = test.get("allow_empty", False)
    min_max_only = test.get("min_max_only", False)

    # Build spec
    spec = {}

    if metrics:
        spec["metrics"] = metrics

    # Build group_by list
    gb_list = []
    for gb in group_bys:
        gb_list.append(classify_group_by(gb))
    for gbo in group_by_objs:
        gb_list.append(classify_group_by_obj(gbo))

    if gb_list:
        spec["group_by"] = gb_list

    # Where filter
    if where_filter:
        rendered_where = render_jinja(where_filter).strip()
        spec["where"] = [rendered_where]

    # Time constraint
    if time_constraint:
        spec["time_constraint"] = time_constraint

    # Order by
    if order_bys:
        spec["order_by"] = order_bys

    # Limit
    if limit:
        spec["limit"] = limit

    # Render check_query
    check_query = render_jinja(check_query_raw).strip() if check_query_raw else ""

    # Skip reason
    skip_reason = determine_skip_reason(test)

    # Already ported
    already_ported = name in ALREADY_PORTED

    result = {
        "name": name,
        "file": filename,
        "model": model,
        "spec": spec,
        "check_query": check_query,
        "check_order": check_order,
        "allow_empty": allow_empty,
        "skip_reason": skip_reason,
        "min_max_only": min_max_only,
    }

    if already_ported:
        result["already_ported"] = True

    return result


def main():
    if not INPUT_DIR.exists():
        print(f"ERROR: Input directory not found: {INPUT_DIR}", file=sys.stderr)
        sys.exit(1)

    yaml_files = sorted(INPUT_DIR.glob("itest_*.yaml"))
    if not yaml_files:
        print(f"ERROR: No itest_*.yaml files found in {INPUT_DIR}", file=sys.stderr)
        sys.exit(1)

    all_tests = []
    names_seen = set()
    skip_counts = {}
    total = 0
    skipped = 0
    already_ported_count = 0

    for yaml_file in yaml_files:
        filename = yaml_file.name
        with open(yaml_file, "r") as f:
            content = f.read()

        # Parse multi-document YAML
        docs = list(yaml.safe_load_all(content))

        for doc in docs:
            if doc is None:
                continue
            if "integration_test" not in doc:
                continue

            test = doc["integration_test"]
            total += 1

            converted = convert_test(test, filename)

            # Dedup check
            if converted["name"] in names_seen:
                # Same name in different files - append file suffix
                # Actually in MetricFlow the same test name shouldn't appear twice,
                # but the metric_queries_no_dimensions file reuses some names like "simple_query"
                original_name = converted["name"]
                suffix = filename.replace("itest_", "").replace(".yaml", "")
                converted["name"] = f"{original_name}__{suffix}"

            names_seen.add(converted["name"])

            if converted.get("skip_reason"):
                skipped += 1
                reason = converted["skip_reason"]
                skip_counts[reason] = skip_counts.get(reason, 0) + 1

            if converted.get("already_ported"):
                already_ported_count += 1

            all_tests.append(converted)

    # Write output
    OUTPUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    with open(OUTPUT_PATH, "w") as f:
        json.dump(all_tests, f, indent=2)

    # Summary
    runnable = total - skipped
    print(f"Total tests found:     {total}")
    print(f"Runnable (no skip):    {runnable}")
    print(f"Skipped:               {skipped}")
    print(f"Already ported:        {already_ported_count}")
    print()
    print("Skip reasons:")
    for reason, count in sorted(skip_counts.items(), key=lambda x: -x[1]):
        print(f"  {reason}: {count}")
    print()
    print(f"Output written to: {OUTPUT_PATH}")


if __name__ == "__main__":
    main()
