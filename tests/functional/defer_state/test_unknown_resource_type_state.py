import json
import os

import pytest

from dbt.tests.util import run_dbt, run_dbt_and_capture, write_file

model_a_sql = "select 1 as id"
model_b_sql = "select * from {{ ref('model_a') }}"


def _check_node(unique_id: str) -> dict:
    # Shape of a check node as written by the dbt v2 engine
    return {
        "resource_type": "check",
        "unique_id": unique_id,
        "name": unique_id.split(".")[-1],
        "package_name": "test",
        "path": "models_have_desc.sql",
        "original_file_path": "checks/models_have_desc.sql",
        "fqn": ["test", "models_have_desc"],
        "raw_code": "select unique_id from {{ info_schema('models') }} where description is null",
        "language": "sql",
        "depends_on": {"macros": [], "nodes": []},
    }


class TestStateManifestWithUnknownResourceType:
    @pytest.fixture(scope="class")
    def models(self):
        return {"model_a.sql": model_a_sql, "model_b.sql": model_b_sql}

    def write_state_with_check(self, project):
        with open(os.path.join(project.project_root, "target", "manifest.json")) as fp:
            manifest = json.load(fp)
        manifest["nodes"]["check.test.models_have_desc"] = _check_node(
            "check.test.models_have_desc"
        )
        manifest["disabled"]["check.test.disabled_check"] = [
            _check_node("check.test.disabled_check")
        ]
        os.makedirs(os.path.join(project.project_root, "state"), exist_ok=True)
        with open(os.path.join(project.project_root, "state", "manifest.json"), "w") as fp:
            json.dump(manifest, fp)

    def test_state_modified_and_defer(self, project):
        run_dbt(["run"])
        self.write_state_with_check(project)

        results, log_output = run_dbt_and_capture(
            ["ls", "--select", "state:modified", "--state", "state"]
        )
        assert results == []
        assert (
            "2 node(s) of resource type 'check' in the manifest are not being used" in log_output
        )

        write_file("select 2 as id", "models", "model_a.sql")
        results = run_dbt(["ls", "--select", "state:modified", "--state", "state"])
        assert results == ["test.model_a"]

        results = run_dbt(
            ["run", "--select", "model_b", "--defer", "--state", "state", "--favor-state"]
        )
        assert len(results) == 1
