"""Use dbt-core itself as the 'v2 parser' to surface hidden parse-phase state.

parse_with_v2 shells out to the v2 parser, then loads
manifest.json and hands it off to later phases. If any phase depends on
in-memory state that ManifestLoader populates but manifest.json doesn't
carry, that phase will misbehave under the v2 flow. To detect this
without a real v2 parser binary, we monkeypatch
dbt.parser.v2._run_v2 so it invokes an in-process `dbt parse`
whose manifest.json lands in the same handoff dir parse_with_v2
created. The rest of parse_with_v2 (WritableManifest load,
build_flat_graph, partial_parse cleanup) runs unchanged.

This trades subprocess fidelity for breadth: it won't catch bugs in
_build_argv flag translation. Pair with a small set of real subprocess
tests (see tests/functional/v2_parser/test_v2_parser_branch.py)
for argv coverage.
"""

from __future__ import annotations

from typing import List

import pytest

from dbt.tests.util import run_dbt, run_dbt_and_capture

_FORWARDED_FLAGS = (
    "--project-dir",
    "--profiles-dir",
    "--profile",
    "--target",
    "--target-path",
    "--packages-install-path",
    "--vars",
)


def _extract(argv: List[str], flag: str):
    if flag in argv:
        return argv[argv.index(flag) + 1]
    return None


def _fake_run_v2(argv: List[str]) -> None:
    parse_args: List[str] = ["parse"]
    for flag in _FORWARDED_FLAGS:
        value = _extract(argv, flag)
        if value is not None:
            parse_args += [flag, value]
    # Guard against recursion: the inner parse must not re-enter the v2 path.
    assert (
        "--use-v2-parser" not in parse_args
    ), "v2_self shim recursed: inner parse argv contained --use-v2-parser"
    run_dbt(parse_args)


def install_shim(monkeypatch) -> None:
    """Activate the v2_self shim for the duration of the calling fixture/test.

    Pass a pytest monkeypatch fixture. Stubs the plugin guards too, since
    Mantle-registered get_nodes plugins fail fast on the v2 branch by
    design and these tests aren't about plugin interop.
    """
    monkeypatch.setattr("dbt.parser.v2._run_v2", _fake_run_v2)
    monkeypatch.setattr("dbt.parser.manifest.assert_no_get_nodes_plugins", lambda *a, **k: None)
    monkeypatch.setattr(
        "dbt.parser.manifest.enrich_manifest_with_plugin_artifacts",
        lambda *a, **k: None,
    )


_V2_FLAGS = ["--use-v2-parser", "--v2-parser=dbt parse"]


def _prepend(parser_mode: str, args):
    # The --v2-parser value is a placeholder; install_shim replaces
    # _run_v2 so the command is never executed, but the CLI flag pair is
    # required for USE_V2_PARSER validation.
    if parser_mode == "v2_self":
        return _V2_FLAGS + list(args)
    return list(args)


def run_dbt_for_mode(parser_mode: str, args: List[str], **kwargs):
    """run_dbt that prepends --use-v2-parser when parser_mode == 'v2_self'."""
    return run_dbt(_prepend(parser_mode, args), **kwargs)


def run_dbt_and_capture_for_mode(parser_mode: str, args: List[str], **kwargs):
    """run_dbt_and_capture variant of run_dbt_for_mode."""
    return run_dbt_and_capture(_prepend(parser_mode, args), **kwargs)


def xfail_v2_self(parser_mode: str, reason: str) -> None:
    """Mark the current test as xfail when running under the v2_self shim.

    Use for tests that hit a known v2-flow divergence documented in
    README.md. xfail (not skip) so that a future fix flips the test to
    xpassed and gets the divergence flagged.
    """
    if parser_mode == "v2_self":
        pytest.xfail(reason)
