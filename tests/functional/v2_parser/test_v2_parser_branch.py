"""End-to-end tests for the --use-v2-parser branch.

We don't depend on a real v2 parser binary. Instead, we run dbt-core's
own parser once to produce a real manifest.json, stash it, then inject a
fake parser script that copies the stash into target/manifest.json on
demand. That gives us a known-good v2-shaped artifact and lets us
assert that dbt-core's load + dispatch logic round-trips through it
correctly.
"""

import shutil
import stat
import sys
from pathlib import Path
from unittest import mock

import pytest

from dbt.tests.util import run_dbt

FAKE_PARSER_PY = '''\
"""Tiny stand-in for the v2 parser. Writes a stashed manifest.json into
whatever --target-path argument we receive (defaults to ./target)."""
import os
import shutil
import sys

STASH = {stash!r}


def main(argv):
    target_path = "target"
    args = iter(argv)
    for arg in args:
        if arg == "--target-path":
            target_path = next(args)
        elif arg == "--project-dir":
            os.chdir(next(args))
        # ignore everything else
    os.makedirs(target_path, exist_ok=True)
    shutil.copy(STASH, os.path.join(target_path, "manifest.json"))


if __name__ == "__main__":
    main(sys.argv[1:])
'''

MODEL_A_SQL = """
select 1 as id
"""

MODEL_B_SQL = """
select * from {{ ref('model_a') }}
"""

SCHEMA_YML = """
version: 2
models:
  - name: model_a
  - name: model_b
"""


class V2ParserFixture:
    @pytest.fixture(autouse=True)
    def _stub_plugin_enrichment(self):
        """Mantle registers global plugins (dbtCloudAutoExposures,
        dbtCloudCrossProjectRef) that advertise `get_nodes`, which the v2
        branch refuses by design. These tests exercise the v2 code path
        itself, not plugin interop, so stub both the fail-fast check and the
        artifact-enrichment hook out."""
        with mock.patch("dbt.parser.manifest.assert_no_get_nodes_plugins"), mock.patch(
            "dbt.parser.manifest.enrich_manifest_with_plugin_artifacts"
        ):
            yield

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "model_a.sql": MODEL_A_SQL,
            "model_b.sql": MODEL_B_SQL,
            "schema.yml": SCHEMA_YML,
        }

    @pytest.fixture(scope="class")
    def fake_parser(self, project, tmp_path_factory):
        """Seed a real manifest.json via core's parser, then build a fake
        v2 parser binary (Python script + thin platform-specific wrapper)
        that copies the stash into <target>/manifest.json on invocation."""
        run_dbt(["parse"])
        seed_path = Path(project.project_root) / "target" / "manifest.json"
        assert seed_path.exists(), "core parse failed to produce manifest.json"

        stash_dir = tmp_path_factory.mktemp("v2_seed")
        stash = stash_dir / "manifest.json"
        shutil.copy(seed_path, stash)

        # Wipe the real manifest so we can prove the fake produced it.
        seed_path.unlink()

        bin_dir = tmp_path_factory.mktemp("v2_bin")
        py_script = bin_dir / "fake_parser.py"
        py_script.write_text(FAKE_PARSER_PY.format(stash=str(stash)))

        python = sys.executable
        if sys.platform == "win32":
            wrapper = bin_dir / "fake_parser.cmd"
            wrapper.write_text(f'@"{python}" "{py_script}" %*\r\n')
        else:
            wrapper = bin_dir / "fake_parser.sh"
            wrapper.write_text(f'#!/usr/bin/env bash\nexec "{python}" "{py_script}" "$@"\n')
            wrapper.chmod(wrapper.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        return wrapper


class TestV2ParserBranch(V2ParserFixture):
    def test_v2_branch_loads_manifest(self, project, fake_parser):
        results = run_dbt(
            [
                "--use-v2-parser",
                f"--v2-parser={fake_parser}",
                "parse",
            ]
        )
        # parse returns the manifest object
        assert results is not None
        assert "model.test.model_a" in results.nodes
        assert "model.test.model_b" in results.nodes

    def test_v2_branch_deletes_stale_partial_parse(self, project, fake_parser):
        target = Path(project.project_root) / "target"
        target.mkdir(exist_ok=True)
        stale = target / "partial_parse.msgpack"
        stale.write_bytes(b"stale-cache")
        run_dbt(
            [
                "--use-v2-parser",
                f"--v2-parser={fake_parser}",
                "parse",
            ]
        )
        assert not stale.exists(), "stale partial_parse.msgpack should be deleted"

    def test_missing_parser_binary_raises(self, project):
        with pytest.raises(Exception, match="(?i)v2 parser|not found"):
            run_dbt(
                [
                    "--use-v2-parser",
                    "--v2-parser=definitely-not-a-real-binary-xyz",
                    "parse",
                ],
                expect_pass=False,
            )

    def test_default_path_unchanged(self, project):
        """With the flag off, parse goes through the regular pipeline."""
        results = run_dbt(["parse"])
        assert results is not None
        assert "model.test.model_a" in results.nodes
