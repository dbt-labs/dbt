import os

import pytest

from dbt.tests.util import get_manifest, run_dbt, write_file

os.environ["DBT_PP_TEST"] = "true"

# ORDERING MATTERS: this bug only triggers when the *source* schema file is processed
# before the snapshot in reverse-sorted file-id order. Both files therefore live under
# models/, and "models/sources.yml" reverse-sorts ahead of "models/a_snap.yml" (s > a),
# so the source is processed first and pre-empts the snapshot's own change detection.
# Defining the snapshot under snapshots/ would reverse-sort it first (the safe order) and
# the test would pass trivially without exercising the bug.

sources_yml = """
version: 2
sources:
  - name: my_source
    schema: my_schema
    tables:
      - name: my_table
"""

sources_yml_changed = """
version: 2
sources:
  - name: my_source
    description: changed
    schema: my_schema
    tables:
      - name: my_table
"""

snapshot_yml = """
snapshots:
  - name: my_snapshot
    relation: "source('my_source', 'my_table')"
    config:
      strategy: check
      unique_key: id
      check_cols: all
      meta:
        owner: team_a
"""

snapshot_yml_changed = snapshot_yml.replace("team_a", "team_b")

SNAP_ID = "snapshot.test.my_snapshot"


class TestSnapshotChangeWithUpstreamSourceCoChange:
    @pytest.fixture(scope="class")
    def models(self):
        return {"sources.yml": sources_yml, "a_snap.yml": snapshot_yml}

    def test_snapshot_change_survives_upstream_source_cochange(self, project):
        # baseline full parse
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        assert manifest.nodes[SNAP_ID].config.meta["owner"] == "team_a"

        # co-change BOTH the snapshot and its upstream source in one partial parse
        write_file(sources_yml_changed, project.project_root, "models", "sources.yml")
        write_file(snapshot_yml_changed, project.project_root, "models", "a_snap.yml")

        # partial parse (reuses the msgpack from the baseline run); pass --partial-parse
        # explicitly so the test can't silently pass via a full-parse fallback
        run_dbt(["--partial-parse", "parse"])
        manifest = get_manifest(project.project_root)
        # FAILS on 1.latest (stays "team_a" - stale node); PASSES after the fix
        assert manifest.nodes[SNAP_ID].config.meta["owner"] == "team_b"


class TestSnapshotChangeInIsolationStillDetected:
    """Regression guard: the direct snapshot-edit path (dbt-core #10907) must keep working."""

    @pytest.fixture(scope="class")
    def models(self):
        return {"sources.yml": sources_yml, "a_snap.yml": snapshot_yml}

    def test_isolated_snapshot_change_detected(self, project):
        run_dbt(["parse"])
        write_file(snapshot_yml_changed, project.project_root, "models", "a_snap.yml")
        # explicit --partial-parse so a full-parse fallback can't mask a regression
        run_dbt(["--partial-parse", "parse"])
        manifest = get_manifest(project.project_root)
        assert manifest.nodes[SNAP_ID].config.meta["owner"] == "team_b"
