#!/usr/bin/env python3
"""
find_upstream.py - Find a Python method in a local dbt-adapters checkout.

The dbt-adapters repo is a monorepo laid out as:
  dbt-adapters/src/dbt/adapters/base/impl.py     -- BaseAdapter
  dbt-adapters/src/dbt/adapters/sql/impl.py      -- SQLAdapter
  dbt-bigquery/src/dbt/adapters/bigquery/impl.py -- BigQueryAdapter
  dbt-snowflake/src/dbt/adapters/snowflake/impl.py
  dbt-redshift/src/dbt/adapters/redshift/impl.py
  dbt-spark/src/dbt/adapters/spark/impl.py
  ... etc.

Databricks and other adapters in separate repos can be added via --extra-repos.

Usage:
    python find_upstream.py <method_name> <dbt_adapters_path> [--adapter-type TYPE]
                            [--extra-repos PATH1,PATH2,...]

Output:
    JSON with {found, method, best, all_matches} where matches is sorted by priority.

Priority (lower = preferred):
  0  Platform-specific impl matching requested adapter_type
  10 sql/impl.py
  20 base/impl.py
  50 Any other impl.py match
"""
import sys
import os
import re
import json
import subprocess
import argparse
from pathlib import Path


# Maps AdapterType variant names (lowercase) to path keyword fragments within
# the monorepo.  The first match wins when scanning platform-specific files.
ADAPTER_TYPE_KEYWORDS = {
    'snowflake':   ['dbt-snowflake', 'snowflake'],
    'bigquery':    ['dbt-bigquery', 'bigquery'],
    'redshift':    ['dbt-redshift', 'redshift'],
    'databricks':  ['databricks'],         # usually in a separate repo
    'spark':       ['dbt-spark', 'spark'],
    'postgres':    ['postgres'],
    'duckdb':      ['duckdb'],
    'fabric':      ['fabric'],
    'salesforce':  ['salesforce'],
    'athena':      ['dbt-athena', 'athena'],
    'trino':       ['trino'],
    'starburst':   ['starburst'],
    'clickhouse':  ['clickhouse'],
}


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
        # Normalise SSH → HTTPS
        url = re.sub(r'^git@github\.com:', 'https://github.com/', url)
        url = re.sub(r'\.git$', '', url)
        return url
    except Exception:
        return 'https://github.com/dbt-labs/dbt-adapters'


AVAILABLE_PATTERN = re.compile(r'^\s*@available(\b|\.)')


def method_line_in_file(path, method_name):
    """
    Return (1-based line number, is_available) for `def <method_name>(` in path, or (None, False).

    is_available is True if any decorator in the block immediately above the def
    matches @available (or @available.parse_list, @available.parse_none, etc.).
    The decorator block starts at the def and walks backward through lines that
    begin with @ or are blank/continuation lines within that decorator block —
    stopping when a non-decorator, non-blank line is hit.
    """
    pattern = re.compile(rf'^\s*def {re.escape(method_name)}\s*\(')
    try:
        with open(path) as f:
            lines = f.readlines()
        for i, line in enumerate(lines):
            if pattern.match(line):
                # Walk backward through the decorator block
                j = i - 1
                is_available = False
                while j >= 0:
                    stripped = lines[j].strip()
                    if not stripped or stripped.startswith('@'):
                        if AVAILABLE_PATTERN.match(lines[j]):
                            is_available = True
                        j -= 1
                    else:
                        break
                return i + 1, is_available  # 1-based
    except Exception:
        pass
    return None, False


def class_at_line(path, target_line):
    """Return the most recent `class Foo` definition before target_line."""
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


def build_url(repo_root, file_path, lineno, sha, remote):
    rel = os.path.relpath(file_path, repo_root)
    return f"{remote}/blob/{sha}/{rel}#L{lineno}"


def search(method_name, repo_paths, adapter_type=None):
    """
    Search all impl.py files across provided repo paths.
    Returns a list of match dicts sorted by priority.
    """
    matches = []
    adapter_type_lc = adapter_type.lower() if adapter_type else None
    platform_keywords = ADAPTER_TYPE_KEYWORDS.get(adapter_type_lc, []) if adapter_type_lc else []

    for repo_path in repo_paths:
        repo_path = Path(repo_path).expanduser().resolve()
        if not repo_path.exists():
            continue

        sha = git_sha(repo_path)
        remote = git_remote_url(repo_path)

        for impl_file in sorted(repo_path.rglob('*impl.py')):
            lineno, is_available = method_line_in_file(impl_file, method_name)
            if lineno is None:
                continue

            path_str = str(impl_file).lower()
            priority = 50  # fallback

            # Platform-specific match?
            if platform_keywords and any(kw in path_str for kw in platform_keywords):
                priority = 0
            elif '/sql/' in path_str or os.sep + 'sql' + os.sep in path_str:
                priority = 10
            elif '/base/' in path_str or os.sep + 'base' + os.sep in path_str:
                priority = 20

            url = build_url(str(repo_path), str(impl_file), lineno, sha, remote) if sha else None
            cls = class_at_line(impl_file, lineno)

            matches.append({
                'priority': priority,
                'file': str(impl_file),
                'line': lineno,
                'class': cls,
                'url': url,
                'sha': sha,
                'remote': remote,
                'is_available': is_available,
            })

    matches.sort(key=lambda x: (x['priority'], x['file']))
    return matches


def main():
    p = argparse.ArgumentParser(description='Find a dbt adapter method upstream.')
    p.add_argument('method_name')
    p.add_argument('dbt_adapters_path', help='Local path to dbt-adapters monorepo checkout')
    p.add_argument('--adapter-type', default=None,
                   help='Adapter type (snowflake, bigquery, …) to prioritise platform-specific impl')
    p.add_argument('--extra-repos', default=None,
                   help='Comma-separated list of extra local repo paths to search (e.g. /path/to/dbt-databricks,/path/to/other)')
                   
    args = p.parse_args()

    repo_paths = [args.dbt_adapters_path]
    if args.extra_repos:
        for path in args.extra_repos.split(','):
            path = path.strip()
            if path:
                repo_paths.append(path)
    if args.databricks_path:
        repo_paths.append(args.databricks_path)

    matches = search(args.method_name, repo_paths, args.adapter_type)

    available_matches = [m for m in matches if m['is_available']]
    result = {
        'found': bool(available_matches),
        'method': args.method_name,
        'best': available_matches[0] if available_matches else None,
        'all_matches': available_matches,
        # Include non-available matches separately for debugging
        'non_available_matches': [m for m in matches if not m['is_available']],
    }
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
