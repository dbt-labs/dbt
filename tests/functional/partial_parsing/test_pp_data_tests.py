import os

import pytest

from dbt.tests.util import get_manifest, run_dbt, write_file

os.environ["DBT_PP_TEST"] = "true"


my_model_sql = """
select 1 as id
"""

schema_test_enabled_yml = """
version: 2

models:
  - name: my_model
    columns:
      - name: id
        data_tests:
          - unique
"""

schema_test_disabled_yml = """
version: 2

models:
  - name: my_model
    columns:
      - name: id
        data_tests:
          - unique:
              config:
                enabled: false
"""


def _find_test_unique_id(manifest):
    prefix = "test.test.unique_my_model_id"
    node_id = next((uid for uid in manifest.nodes if uid.startswith(prefix)), None)
    if node_id:
        return node_id, "nodes"
    disabled_id = next((uid for uid in manifest.disabled if uid.startswith(prefix)), None)
    if disabled_id:
        return disabled_id, "disabled"
    return None, None


class TestPartialParsingDataTestEnabledToggle:
    """CORE-725 MODE 1: a column-level generic test whose `enabled` config is
    toggled must not accumulate ghost copies in manifest.disabled across
    partial parses, and must never raise a duplicate-data_tests error."""

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "my_model.sql": my_model_sql,
            "schema.yml": schema_test_enabled_yml,
        }

    def test_data_test_enabled_toggle_no_ghost_accumulation(self, project):
        # initial parse: test enabled
        manifest = run_dbt(["parse"])
        test_unique_id, location = _find_test_unique_id(manifest)
        assert test_unique_id is not None
        assert location == "nodes"
        assert test_unique_id not in manifest.disabled

        # toggle enabled -> disabled -> enabled -> disabled several times.
        # On the buggy code, the disabled entry accumulates ghost copies
        # because remove_tests never pops the stale node from manifest.disabled.
        for _ in range(3):
            # disable the test
            write_file(schema_test_disabled_yml, project.project_root, "models", "schema.yml")
            run_dbt(["--partial-parse", "parse"])
            manifest = get_manifest(project.project_root)
            test_unique_id, location = _find_test_unique_id(manifest)
            assert test_unique_id is not None
            assert test_unique_id not in manifest.nodes
            # the disabled node must not accumulate ghost copies
            assert len(manifest.disabled.get(test_unique_id, [])) <= 1

            # re-enable the test
            write_file(schema_test_enabled_yml, project.project_root, "models", "schema.yml")
            run_dbt(["--partial-parse", "parse"])
            manifest = get_manifest(project.project_root)
            test_unique_id, location = _find_test_unique_id(manifest)
            assert test_unique_id is not None
            assert location == "nodes"
            assert len(manifest.disabled.get(test_unique_id, [])) == 0
