#!/usr/bin/env python3
"""
scan_available.py — Enumerate all @available-decorated methods in the dbt-adapters Python chain.

Walks:
  base/impl.py      (priority 20, BaseAdapter)
  sql/impl.py       (priority 10, SQLAdapter)
  platform/impl.py  (priority  0, when --adapter-type is given)
  extra repos       (priority  0 if platform-matched, else 50)

Returns a JSON object keyed by method name; each value is a list of matches
sorted by priority (lowest = most specific).  Methods without any @available
occurrence are excluded entirely.

Usage:
    python3 scan_available.py <dbt_adapters_path> \\
        [--adapter-type TYPE] \\
        [--extra-repos PATH1,PATH2,...] \\
        [--all-adapters]

    --all-adapters  scan every platform-specific impl.py in the monorepo
                    (ignores --adapter-type; returns everything)
"""

import sys
import re
import os
import json
import argparse
import subprocess
from pathlib import Path
from collections import defaultdict

AVAILABLE_PATTERN = re.compile(r'^\s*@available(\b|\.)')

ADAPTER_TYPE_KEYWORDS = {
    'snowflake':  ['dbt-snowflake', 'snowflake'],
    'bigquery':   ['dbt-bigquery', 'bigquery'],
    'redshift':   ['dbt-redshift', 'redshift'],
    'databricks': ['databricks'],
    'spark':      ['dbt-spark', 'spark'],
    'postgres':   ['postgres'],
    'duckdb':     ['duckdb'],
    'fabric':     ['fabric'],
    'salesforce': ['salesforce'],
    'athena':     ['dbt-athena', 'athena'],
    'trino':      ['trino'],
    'starburst':  ['starburst'],
    'clickhouse': ['clickhouse'],
}


# ── Git helpers ───────────────────────────────────────────────────────────────

def git_sha(repo_path):
    try:
        r = subprocess.run(
            ['git', 'rev-parse', 'HEAD'],
            cwd=repo_path, capture_output=True, text=True, check=True,
        )
        return r.stdout.strip()
    except Exception:
        return None


def git_remote_url(repo_path):
    try:
        r = subprocess.run(
            ['git', 'remote', 'get-url', 'origin'],
            cwd=repo_path, capture_output=True, text=True, check=True,
        )
        url = r.stdout.strip()
        url = re.sub(r'^git@github\.com:', 'https://github.com/', url)
        return re.sub(r'\.git$', '', url)
    except Exception:
        return 'https://github.com/dbt-labs/dbt-adapters'


def build_url(repo_root, file_path, lineno, sha, remote):
    rel = os.path.relpath(str(file_path), str(repo_root))
    return f"{remote}/blob/{sha}/{rel}#L{lineno}"


def class_at_line(path, target_line):
    """Return the name of the innermost `class Foo` definition before target_line."""
    pattern = re.compile(r'^class (\w+)')
    best = None
    try:
        with open(path) as f:
            for lineno, line in enumerate(f, 1):
                if lineno >= target_line:
                    break
                m = pattern.match(line)
                if m:
                    best = m.group(1)
    except Exception:
        pass
    return best


# ── Core scanner ──────────────────────────────────────────────────────────────

def scan_impl_file(impl_file, repo_root, sha, remote, adapter_type_lc, all_adapters):
    """
    Scan a single *impl.py file for @available methods.

    Returns list of match dicts:
        method, file, line (1-based), class, url, priority
    """
    path_str = str(impl_file).lower()
    platform_keywords = ADAPTER_TYPE_KEYWORDS.get(adapter_type_lc, []) if adapter_type_lc else []

    # Assign priority for this file
    if all_adapters:
        # Treat any platform-specific file as priority 0
        if '/sql/' in path_str or os.sep + 'sql' + os.sep in path_str:
            priority = 10
        elif '/base/' in path_str or os.sep + 'base' + os.sep in path_str:
            priority = 20
        else:
            priority = 0   # all platform-specific files treated equally
    elif platform_keywords and any(kw in path_str for kw in platform_keywords):
        priority = 0
    elif '/sql/' in path_str or os.sep + 'sql' + os.sep in path_str:
        priority = 10
    elif '/base/' in path_str or os.sep + 'base' + os.sep in path_str:
        priority = 20
    else:
        priority = 50   # other impl.py not matching the requested platform

    results = []
    try:
        with open(impl_file) as f:
            lines = f.readlines()
    except Exception:
        return results

    DEF_PATTERN = re.compile(r'^\s*def (\w+)\s*\(')

    for i, line in enumerate(lines):
        m = DEF_PATTERN.match(line)
        if not m:
            continue
        method_name = m.group(1)
        if method_name.startswith('_'):
            continue   # skip private/dunder methods

        # Walk backward through the decorator block
        j = i - 1
        is_available = False
        while j >= 0:
            stripped = lines[j].strip()
            if stripped.startswith('@'):
                if AVAILABLE_PATTERN.match(lines[j]):
                    is_available = True
                j -= 1
            elif not stripped:
                j -= 1
            else:
                break

        if not is_available:
            continue

        lineno = i + 1  # 1-based
        cls = class_at_line(impl_file, lineno)
        url = build_url(str(repo_root), str(impl_file), lineno, sha, remote) if sha else None

        results.append({
            'method':   method_name,
            'file':     str(impl_file),
            'line':     lineno,
            'class':    cls,
            'url':      url,
            'priority': priority,
        })

    return results


def collect_available_methods(repo_paths, adapter_type=None, all_adapters=False):
    """
    Scan all *impl.py files across repo_paths for @available methods.

    Returns:
        dict[method_name → list[match_dict]]  (matches sorted by priority asc)
    """
    adapter_type_lc = adapter_type.lower() if adapter_type else None
    all_matches = defaultdict(list)

    for repo_path in repo_paths:
        repo_path = Path(repo_path).expanduser().resolve()
        if not repo_path.exists():
            print(f"Warning: repo path not found: {repo_path}", file=sys.stderr)
            continue

        sha     = git_sha(repo_path)
        remote  = git_remote_url(repo_path)

        for impl_file in sorted(repo_path.rglob('*impl.py')):
            matches = scan_impl_file(
                impl_file, repo_path, sha, remote, adapter_type_lc, all_adapters,
            )
            for m in matches:
                all_matches[m['method']].append(m)

    # Sort each method's matches by priority (lowest = most specific)
    for method in all_matches:
        all_matches[method].sort(key=lambda x: (x['priority'], x['file']))

    return dict(all_matches)


# ── CLI ───────────────────────────────────────────────────────────────────────

def main():
    p = argparse.ArgumentParser(
        description='Enumerate @available methods in the dbt-adapters Python chain.',
    )
    p.add_argument('dbt_adapters_path', help='Path to dbt-adapters monorepo checkout')
    p.add_argument('--adapter-type', default=None,
                   help='Adapter type to prioritise (snowflake, bigquery, …)')
    p.add_argument('--extra-repos', default=None,
                   help='Comma-separated extra repo paths (e.g. /path/to/dbt-databricks)')
    p.add_argument('--all-adapters', action='store_true',
                   help='Include all platform-specific impl.py files, not just one adapter')
    args = p.parse_args()

    repo_paths = [args.dbt_adapters_path]
    if args.extra_repos:
        for rp in args.extra_repos.split(','):
            rp = rp.strip()
            if rp:
                repo_paths.append(rp)

    methods = collect_available_methods(
        repo_paths,
        adapter_type=args.adapter_type,
        all_adapters=args.all_adapters,
    )

    print(f"Found {len(methods)} unique @available method names", file=sys.stderr)
    print(json.dumps(methods, indent=2))


if __name__ == '__main__':
    main()
