#!/usr/bin/env python3
"""
annotate.py — Annotate adapter_impl.rs with upstream dbt-adapters Python reference links.

Walks the dbt-adapters Python inheritance chain (BaseAdapter → SQLAdapter →
[AdapterType]Adapter), collects every @available-decorated method, then:

  • Found in adapter_impl.rs AND not yet annotated
        → inserts  /// ClassName <github-url>  as a doc comment
  • Found in adapter_impl.rs AND already annotated
        → skips (idempotent)
  • NOT found in adapter_impl.rs
        → records in a markdown gap report

Usage:
    python3 annotate.py \\
        --adapter-impl  <path/to/adapter_impl.rs>         \\
        --dbt-adapters  <path/to/dbt-adapters-checkout>   \\
        [--adapter-type  TYPE]                             \\
        [--all-adapters]                                   \\
        [--extra-repos   PATH1,PATH2,...]                  \\
        [--missing-out   PATH]   (default: missing_adapter_methods.md) \\
        [--dry-run]

Examples:
    # All @available methods for BigQuery
    python3 annotate.py \\
        --adapter-impl crates/dbt-adapter/src/adapter/adapter_impl.rs \\
        --dbt-adapters ~/code/dbt-adapters \\
        --adapter-type bigquery

    # Every adapter at once (full coverage)
    python3 annotate.py \\
        --adapter-impl crates/dbt-adapter/src/adapter/adapter_impl.rs \\
        --dbt-adapters ~/code/dbt-adapters \\
        --all-adapters \\
        --extra-repos ~/code/dbt-databricks \\
        --missing-out missing_adapter_methods.md
"""

import sys
import re
import os
import json
import argparse
from pathlib import Path
from collections import defaultdict

# Reuse shared helpers from the scripts package
# (scan_available.py must be on the path or in the same directory)
_scripts_dir = Path(__file__).parent
sys.path.insert(0, str(_scripts_dir))
from scan_available import collect_available_methods  # noqa: E402

# ── Patterns ──────────────────────────────────────────────────────────────────

PUB_FN_PATTERN           = re.compile(r'^(\s*)pub fn (\w+)\s*[<(]')
CANONICAL_ANN_PATTERN    = re.compile(r'^\s*///\s+\w+\s+https://github\.com/')
DOC_COMMENT_PATTERN      = re.compile(r'^\s*///')
ATTR_PATTERN             = re.compile(r'^\s*#\[')


# ── Rust file scanning ────────────────────────────────────────────────────────

def scan_rust_methods(adapter_impl_rs):
    """
    Returns dict: method_name → {line (1-based), indent, has_annotation, canonical_lines}

    has_annotation is True if the preceding /// block contains at least one
    canonical "/// ClassName https://github.com/..." line.
    canonical_lines is the list of 0-based indices of ALL such lines (a method
    may have multiple, e.g. BaseAdapter + SnowflakeAdapter + DatabricksAdapter).
    Used by --force to refresh each one independently.
    """
    rust_methods = {}
    try:
        with open(adapter_impl_rs) as f:
            lines = f.readlines()
    except Exception as e:
        print(f"Error reading {adapter_impl_rs}: {e}", file=sys.stderr)
        return {}, []

    for i, line in enumerate(lines):
        m = PUB_FN_PATTERN.match(line)
        if not m:
            continue
        indent, method_name = m.group(1), m.group(2)

        # Gather `/// doc lines`
        # walk backward past #[...] attrs, blank lines preceding the function definition
        # recording both the stripped text and the original line index.
        j = i - 1
        doc_items = []   # list of (stripped_line, original_0based_index)
        while j >= 0:
            stripped = lines[j].strip()
            if stripped.startswith('///'):
                doc_items.append((stripped, j))
                j -= 1
            elif stripped.startswith('#[') or stripped == '':
                j -= 1
            else:
                break

        canonical_lines = [idx for stripped, idx in doc_items
                           if CANONICAL_ANN_PATTERN.match(stripped)]

        rust_methods[method_name] = {
            'line':            i + 1,
            'indent':          indent,
            'has_annotation':  bool(canonical_lines),
            'canonical_lines': canonical_lines,   # all 0-based indices of existing annotations
        }

    return rust_methods, lines


# ── Insert-position calculation ───────────────────────────────────────────────

def find_insert_position(lines, pub_fn_line_1based):
    """
    Return (insert_index, needs_separator) where:
      insert_index    0-based index to insert new lines before
      needs_separator True if a blank '///' line should precede the annotation

    Logic:
      - Walk backward from pub fn past #[...] attrs and blank lines.
      - If we land on a /// line: insert after it (append to doc block).
        Add a blank '///' separator unless the last doc line is already blank.
      - Otherwise: insert before the first #[...] attr (or before pub fn if none).
    """
    pub_fn_idx = pub_fn_line_1based - 1   # 0-based

    # Step 1: skip backward past #[...] attrs and blank lines
    j = pub_fn_idx - 1
    first_attr_idx = None
    while j >= 0:
        stripped = lines[j].strip()
        if stripped.startswith('#['):
            first_attr_idx = j
            j -= 1
        elif stripped == '':
            j -= 1
        else:
            break

    if j >= 0 and lines[j].strip().startswith('///'):
        # There's an existing doc comment block.  Insert after it.
        last_doc_idx = j
        # Does the doc block end with a blank '///' line already?
        needs_separator = (lines[last_doc_idx].strip() != '///')
        return last_doc_idx + 1, needs_separator
    else:
        # No doc comment.  Insert before the first #[...] attr (if any).
        if first_attr_idx is not None:
            return first_attr_idx, False
        else:
            return pub_fn_idx, False


# ── Annotation application ────────────────────────────────────────────────────

def build_annotation_lines(indent, cls, url, needs_separator):
    """Return list of lines to insert (each already ending with \\n)."""
    result = []
    if needs_separator:
        result.append(f"{indent}///\n")
    result.append(f"{indent}/// {cls} {url}\n")
    return result


def apply_annotations(adapter_impl_rs, insertions, lines, dry_run=False):
    """
    insertions: list of {method, insert_at (0-based), new_lines, replace_line (0-based or None)}

    When replace_line is set (--force path), the existing canonical annotation line is
    replaced in place instead of inserting new lines.  All other insertions are applied
    bottom-to-top so earlier line numbers don't shift.
    """
    insertions_sorted = sorted(insertions, key=lambda a: a['insert_at'], reverse=True)
    for ins in insertions_sorted:
        replace = ins.get('replace_line')
        if replace is not None:
            if dry_run:
                print(f"  DRY-RUN REPLACE @{replace+1}: {ins['new_lines'][0].rstrip()}")
            else:
                lines[replace] = ins['new_lines'][0]
        else:
            if dry_run:
                for nl in ins['new_lines']:
                    print(f"  DRY-RUN INSERT  @{ins['insert_at']+1}: {nl.rstrip()}")
            else:
                for i, nl in enumerate(ins['new_lines']):
                    lines.insert(ins['insert_at'] + i, nl)

    if not dry_run:
        with open(adapter_impl_rs, 'w') as f:
            f.writelines(lines)


# ── Missing report ────────────────────────────────────────────────────────────

# In-house adapters maintained by dbt-labs, in canonical display order.
# Always rendered as sections in this order; ✅ shown when nothing is missing.
CANONICAL_ADAPTERS = [
    ('snowflake',  'Snowflake'),
    ('bigquery',   'BigQuery'),
    ('redshift',   'Redshift'),
    ('spark',      'Spark'),
    ('postgres',   'Postgres'),
    ('athena',     'Athena'),
]


def classify_platform(py_info):
    """Return a lowercase platform key for a Python match (e.g. 'snowflake', 'athena')."""
    cls  = (py_info.get('class') or '').lower()
    path = (py_info.get('file')  or '').lower()
    for key, _ in CANONICAL_ADAPTERS:
        if key in cls or key in path:
            return key
    if 'databricks' in cls or 'databricks' in path:
        return 'databricks'
    # Generic fallback: strip 'adapter' suffix from class name (e.g. 'clickhouseadapter' → 'clickhouse')
    if cls.endswith('adapter') and len(cls) > len('adapter'):
        return cls[:-len('adapter')]
    return 'other'


def write_missing_report(missing_methods, adapter_type, output_path, dry_run=False):
    """
    missing_methods: dict[name → {python: best_match, all_python: [...]}]

    Renders sections in canonical adapter order (Snowflake → BigQuery → Redshift → Spark →
    Postgres → Athena) with ✅ for adapters that have no gaps, then any remaining adapters,
    then Base / SQL Adapter.
    """
    # Bucket platform-specific (priority 0) vs base/SQL (priority > 0)
    platform_buckets = {}   # platform_key → {method_name: entry}
    base_sql_missing = {}   # method_name → entry

    for name, v in missing_methods.items():
        best = v['python']
        if best['priority'] == 0:
            key = classify_platform(best)
            platform_buckets.setdefault(key, {})[name] = v
        else:
            base_sql_missing[name] = v

    # Adapters discovered but not in the canonical list (e.g. Databricks)
    canonical_keys  = {k for k, _ in CANONICAL_ADAPTERS}
    extra_adapters  = sorted(k for k in platform_buckets if k not in canonical_keys)

    def method_rows(methods_dict):
        rows = [
            "| Method | Python class | Source |\n",
            "|--------|-------------|--------|\n",
        ]
        for name in sorted(methods_dict):
            py  = methods_dict[name]['python']
            cls = py['class'] or '?'
            url = py['url'] or py['file']
            rows.append(f"| `{name}` | `{cls}` | [link]({url}) |\n")
        return rows

    out = [
        "# Missing Adapter Methods\n",
        "\n",
        "Methods decorated with `@available` in the Python dbt-adapters inheritance chain\n",
        "that have no corresponding `pub fn` in `adapter_impl.rs`.\n",
        "\n",
    ]

    # Canonical adapters — always rendered; ✅ when no gaps
    for key, label in CANONICAL_ADAPTERS:
        out.append(f"## {label}\n\n")
        if key in platform_buckets:
            out += method_rows(platform_buckets[key])
        else:
            out.append("✅ No missing methods\n")
        out.append("\n")

    # Non-canonical adapters (e.g. Databricks, ClickHouse)
    for key in extra_adapters:
        label = key.capitalize()
        out.append(f"## {label}\n\n")
        out += method_rows(platform_buckets[key])
        out.append("\n")

    # Base / SQL Adapter
    out.append("## Base / SQL Adapter\n\n")
    if base_sql_missing:
        out += method_rows(base_sql_missing)
    else:
        out.append("✅ No missing methods\n")
    out.append("\n")

    if dry_run:
        print(f"\nDRY-RUN: would write {len(missing_methods)} missing methods to {output_path}",
              file=sys.stderr)
        return

    with open(output_path, 'w') as f:
        f.writelines(out)
    print(f"Missing methods report → {output_path}  ({len(missing_methods)} methods)",
          file=sys.stderr)


# ── Main ──────────────────────────────────────────────────────────────────────

def main():
    p = argparse.ArgumentParser(
        description='Reverse-annotate adapter_impl.rs from Python @available methods.',
    )
    p.add_argument('--adapter-impl', required=True,
                   help='Path to adapter_impl.rs')
    p.add_argument('--dbt-adapters', required=True,
                   help='Path to dbt-adapters monorepo checkout')
    p.add_argument('--adapter-type', default=None,
                   help='Adapter type (snowflake, bigquery, …) to include platform-specific methods')
    p.add_argument('--all-adapters', action='store_true',
                   help='Include every platform-specific impl.py (overrides --adapter-type scope)')
    p.add_argument('--extra-repos', default=None,
                   help='Comma-separated extra repo paths (e.g. /path/to/dbt-databricks)')
    p.add_argument('--missing-out', default='missing_adapter_methods.md',
                   help='Output path for missing-methods report (default: missing_adapter_methods.md)')
    p.add_argument('--dry-run', action='store_true',
                   help='Print planned changes without modifying any files')
    p.add_argument('--force', action='store_true',
                   help='Re-annotate methods that already have a canonical annotation, '
                        'replacing the existing URL in place. Use when updating to a new '
                        'dbt-adapters checkout and want all SHAs refreshed.')
    args = p.parse_args()

    repo_paths = [args.dbt_adapters]
    if args.extra_repos:
        for rp in args.extra_repos.split(','):
            rp = rp.strip()
            if rp:
                repo_paths.append(rp)

    # ── 1. Collect Python @available methods ──────────────────────────────────
    label = 'all adapters' if args.all_adapters else (args.adapter_type or 'base + sql only')
    print(f"Scanning Python @available methods ({label})…", file=sys.stderr)
    python_methods = collect_available_methods(
        repo_paths,
        adapter_type=args.adapter_type,
        all_adapters=args.all_adapters,
    )
    print(f"  {len(python_methods)} unique @available method names", file=sys.stderr)

    # ── 2. Scan Rust ──────────────────────────────────────────────────────────
    print(f"Scanning Rust methods in {args.adapter_impl}…", file=sys.stderr)
    rust_methods, rust_lines = scan_rust_methods(args.adapter_impl)
    print(f"  {len(rust_methods)} pub fn methods", file=sys.stderr)

    # ── 3. Classify ───────────────────────────────────────────────────────────
    found_methods   = {}
    missing_methods = {}

    for method_name, py_matches in sorted(python_methods.items()):
        best = py_matches[0]   # lowest priority = most platform-specific
        if method_name in rust_methods:
            found_methods[method_name] = {
                'rust':       rust_methods[method_name],
                'python':     best,
                'all_python': py_matches,
            }
        else:
            missing_methods[method_name] = {
                'python':     best,
                'all_python': py_matches,
            }

    already_ok       = sum(1 for v in found_methods.values() if v['rust']['has_annotation'])
    needs_annotating = {k: v for k, v in found_methods.items() if not v['rust']['has_annotation']}
    # With --force, also re-annotate methods that already have a (possibly stale) annotation.
    force_update     = {k: v for k, v in found_methods.items() if v['rust']['has_annotation']} \
                       if args.force else {}

    print(f"\nResults:", file=sys.stderr)
    print(f"  {len(found_methods)} found in Rust ({already_ok} already annotated)", file=sys.stderr)
    if args.force:
        print(f"  {len(force_update)} to refresh (--force)", file=sys.stderr)
    print(f"  {len(needs_annotating)} to annotate", file=sys.stderr)
    print(f"  {len(missing_methods)} missing from Rust", file=sys.stderr)

    # ── 4. Build insertions ───────────────────────────────────────────────────
    insertions = []

    # New annotations (no existing canonical line)
    for method_name, info in needs_annotating.items():
        best      = info['python']
        rust_info = info['rust']
        if not best['url']:
            print(f"  Skipping {method_name}: no URL (no git SHA?)", file=sys.stderr)
            continue
        cls = best['class'] or 'BaseAdapter'
        insert_at, needs_sep = find_insert_position(rust_lines, rust_info['line'])
        insertions.append({
            'method':       method_name,
            'insert_at':    insert_at,
            'new_lines':    build_annotation_lines(rust_info['indent'], cls, best['url'], needs_sep),
            'replace_line': None,
        })

    # Force-refresh: update the URL of each existing canonical annotation, keeping class names.
    # A method may have multiple /// annotations (e.g. BaseAdapter + SnowflakeAdapter), so we
    # iterate over all canonical lines and refresh each one independently.
    # We match by class name, not by priority order, to avoid re-classification churn.
    existing_cls_pattern = re.compile(r'^\s*///\s+(\w+)\s+https://')
    for method_name, info in force_update.items():
        rust_info = info['rust']
        for replace_line in rust_info['canonical_lines']:
            existing_line = rust_lines[replace_line]
            m = existing_cls_pattern.match(existing_line)
            if not m:
                continue
            existing_cls = m.group(1)

            # Find the Python match whose class equals the existing annotation class
            same_class_match = next(
                (p for p in info['all_python'] if p.get('class') == existing_cls),
                None,
            )
            if same_class_match is None or not same_class_match.get('url'):
                # Class no longer has @available or no URL — leave untouched
                continue

            new_line = f"{rust_info['indent']}/// {existing_cls} {same_class_match['url']}\n"
            if new_line == existing_line:
                continue   # already up to date; skip to avoid noise in git diff
            insertions.append({
                'method':       method_name,
                'insert_at':    replace_line,
                'new_lines':    [new_line],
                'replace_line': replace_line,
            })

    # ── 5. Apply annotations ──────────────────────────────────────────────────
    if insertions:
        print(f"\nAnnotating {len(insertions)} methods…", file=sys.stderr)
        for ins in sorted(insertions, key=lambda x: x['insert_at']):
            print(f"  + {ins['method']}", file=sys.stderr)
        apply_annotations(args.adapter_impl, insertions, rust_lines, dry_run=args.dry_run)
        if not args.dry_run:
            print("  Done.", file=sys.stderr)
    else:
        print("\nNothing to annotate.", file=sys.stderr)

    # ── 6. Missing methods report ─────────────────────────────────────────────
    if missing_methods:
        write_missing_report(
            missing_methods, label, args.missing_out, dry_run=args.dry_run,
        )

    # ── Summary (stdout JSON) ─────────────────────────────────────────────────
    platform_missing = sum(
        1 for v in missing_methods.values() if v['python']['priority'] == 0
    )
    print(json.dumps({
        'annotated':                len(insertions),
        'already_annotated':        already_ok,
        'missing_from_rust':        len(missing_methods),
        'missing_platform_specific': platform_missing,
        'missing_base_or_sql':      len(missing_methods) - platform_missing,
    }, indent=2))


if __name__ == '__main__':
    main()
