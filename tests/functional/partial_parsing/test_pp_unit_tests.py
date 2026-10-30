import os

import pytest

from dbt.exceptions import ParsingError
from dbt.tests.util import get_manifest, rm_file, run_dbt, write_file


def normalize(path):
    return os.path.normcase(os.path.normpath(path))


model_one_sql = """
select 1 as fun
"""

# --- scenario 1: version-set change, unit-test file untouched ---

unversioned_schema_yml = """
models:
    - name: model_one
"""

versioned_schema_yml = """
models:
    - name: model_one
      latest_version: 1
      versions:
        - v: 1
"""

unit_test_yml = """
unit_tests:
    - name: test_model_one
      model: model_one
      given: []
      expect:
        rows:
          - {fun: 1}
"""


class TestVersionChangeUnitTestFileUntouched:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "model_one.sql": model_one_sql,
            "schema.yml": unversioned_schema_yml,
            "unit_tests.yml": unit_test_yml,
        }

    def test_pp_version_change_reresolves_unit_test(self, project):
        manifest = run_dbt(["parse"])
        assert "unit_test.test.model_one.test_model_one" in manifest.unit_tests
        unit_test = manifest.unit_tests["unit_test.test.model_one.test_model_one"]
        assert unit_test.depends_on.nodes == ["model.test.model_one"]

        # Add versioning to model_one (v1 is latest, so it defaults to the
        # existing model_one.sql -- no file rename needed); the unit-test
        # YAML is untouched.
        write_file(versioned_schema_yml, project.project_root, "models", "schema.yml")
        manifest = run_dbt(["--partial-parse", "parse"])

        assert "unit_test.test.model_one.test_model_one_v1" in manifest.unit_tests
        versioned_unit_test = manifest.unit_tests["unit_test.test.model_one.test_model_one_v1"]
        assert versioned_unit_test.depends_on.nodes == ["model.test.model_one.v1"]


# --- scenario 2: tested model disabled on a later partial-parse pass ---

model_and_unit_test_enabled_yml = """
models:
    - name: model_one

unit_tests:
    - name: test_model_one
      model: model_one
      description: "v1"
      given: []
      expect:
        rows:
          - {fun: 1}
"""

# Disables the model AND edits the unit test's own description. dbt's
# partial-parse schema diffing reschedules a unit test for reparsing based on
# its own schema element changing, or (separately) on the tested model's
# group/version changing (see partial.py _delete_schema_mssa_links) -- not on
# an unrelated model's `enabled` config alone changing in a shared file. This
# test exercises the parse-order fix via the former path; the latter is a
# distinct, pre-existing gap tracked separately.
model_disabled_yml = """
models:
    - name: model_one
      config:
        enabled: false

unit_tests:
    - name: test_model_one
      model: model_one
      description: "v2"
      given: []
      expect:
        rows:
          - {fun: 1}
"""


class TestModelDisabledOnLaterPartialParse:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "model_one.sql": model_one_sql,
            "schema.yml": model_and_unit_test_enabled_yml,
        }

    def test_pp_model_disabled_moves_unit_test_once(self, project):
        manifest = run_dbt(["parse"])
        unit_test_id = "unit_test.test.model_one.test_model_one"
        assert unit_test_id in manifest.unit_tests
        assert manifest.unit_tests[unit_test_id].depends_on.nodes == ["model.test.model_one"]

        # Disable the tested model. The unit test is defined in the same
        # schema.yml file, so this edit reparses both.
        write_file(model_disabled_yml, project.project_root, "models", "schema.yml")
        run_dbt(["--partial-parse", "parse"])
        manifest = get_manifest(project.project_root)

        assert unit_test_id not in manifest.unit_tests
        assert unit_test_id in manifest.disabled
        # Regression check for the add_disabled/add_disabled_nofile double-append fix.
        assert len(manifest.disabled[unit_test_id]) == 1
        schema_file = manifest.files["test://" + normalize("models/schema.yml")]
        assert schema_file.unit_tests.count(unit_test_id) == 1


# --- scenario 3: model delete/re-add ---


class TestModelDeleteReAdd:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "model_one.sql": model_one_sql,
            "schema.yml": unversioned_schema_yml,
            "unit_tests.yml": unit_test_yml,
        }

    def test_pp_model_delete_readd(self, project):
        manifest = run_dbt(["parse"])
        assert "unit_test.test.model_one.test_model_one" in manifest.unit_tests

        # Delete the model entirely (file + schema entry).
        rm_file(project.project_root, "models", "model_one.sql")
        write_file("models: []\n", project.project_root, "models", "schema.yml")
        with pytest.raises(ParsingError):
            run_dbt(["--partial-parse", "parse"])

        # Re-add the model and confirm clean re-resolution with no dupes.
        write_file(model_one_sql, project.project_root, "models", "model_one.sql")
        write_file(unversioned_schema_yml, project.project_root, "models", "schema.yml")
        manifest = run_dbt(["--partial-parse", "parse"])

        unit_test_id = "unit_test.test.model_one.test_model_one"
        assert unit_test_id in manifest.unit_tests
        assert manifest.unit_tests[unit_test_id].depends_on.nodes == ["model.test.model_one"]
        schema_file = manifest.files["test://" + normalize("models/unit_tests.yml")]
        assert schema_file.unit_tests.count(unit_test_id) == 1
