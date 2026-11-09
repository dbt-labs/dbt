import json
import os

import pytest

from dbt.tests.util import run_dbt, update_config_file

model_with_hooks_sql = """
{{ config(pre_hook=["select 1"], post_hook="select 2") }}
select 1 as id
"""


class TestUnchangedHooksNotModifiedAgainstOlderManifest:
    """Without state_modified_compare_more_unrendered_values, the default before dbt 1.10, the
    manifest stored hooks under "pre-hook" and "post-hook". Comparing against such a manifest
    with the flag on must not select a model with hooks whose code did not change."""

    @pytest.fixture(scope="class")
    def models(self):
        return {"model_with_hooks.sql": model_with_hooks_sql}

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"flags": {"state_modified_compare_more_unrendered_values": False}}

    def test_unchanged_model_with_hooks_is_not_modified(self, project):
        run_dbt(["parse"])

        with open(os.path.join("target", "manifest.json")) as f:
            manifest = json.load(f)
        unrendered_config = manifest["nodes"]["model.test.model_with_hooks"]["unrendered_config"]
        assert "pre-hook" in unrendered_config
        assert "post-hook" in unrendered_config

        # make the state look like it was written by an older dbt version, so it is upgraded
        manifest["metadata"]["dbt_version"] = "PREVIOUS"
        os.makedirs("state")
        with open(os.path.join("state", "manifest.json"), "w") as f:
            json.dump(manifest, f)

        update_config_file(
            {"flags": {"state_modified_compare_more_unrendered_values": True}},
            project.project_root,
            "dbt_project.yml",
        )

        results = run_dbt(["ls", "--select", "state:modified", "--state", "./state"])
        assert len(results) == 0
