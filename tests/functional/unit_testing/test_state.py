import os
import pytest
import shutil
from copy import deepcopy

from dbt.tests.util import (
    run_dbt,
    write_file,
    write_config_file,
)
from fixtures import (
    my_model_vars_sql,
    my_model_a_sql,
    my_model_b_sql,
    test_my_model_simple_fixture_yml,
    test_my_model_fixture_csv,
    test_my_model_b_fixture_csv,
)


class UnitTestState:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "my_model.sql": my_model_vars_sql,
            "my_model_a.sql": my_model_a_sql,
            "my_model_b.sql": my_model_b_sql,
            "test_my_model.yml": test_my_model_simple_fixture_yml,
        }

    @pytest.fixture(scope="class")
    def tests(self):
        return {
            "fixtures": {
                "test_my_model_fixture.csv": test_my_model_fixture_csv,
            }
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"vars": {"my_test": "my_test_var"}}

    def copy_state(self, project_root):
        state_path = os.path.join(project_root, "state")
        if not os.path.exists(state_path):
            os.makedirs(state_path)
        shutil.copyfile(
            f"{project_root}/target/manifest.json", f"{project_root}/state/manifest.json"
        )
        shutil.copyfile(
            f"{project_root}/target/run_results.json", f"{project_root}/state/run_results.json"
        )


class TestUnitTestStateModified(UnitTestState):
    def test_state_modified(self, project):
        run_dbt(["run"])
        run_dbt(["test"], expect_pass=False)
        self.copy_state(project.project_root)

        # no changes
        results = run_dbt(["test", "--select", "state:modified", "--state", "state"])
        assert len(results) == 0

        # change underlying fixture file
        write_file(
            test_my_model_b_fixture_csv,
            project.project_root,
            "tests",
            "fixtures",
            "test_my_model_fixture.csv",
        )
        results = run_dbt(
            ["test", "--select", "state:modified", "--state", "state"], expect_pass=True
        )
        assert len(results) == 1
        assert results[0].node.name.endswith("test_depends_on_fixture")
        # reset changes
        self.copy_state(project.project_root)

        # change unit test definition of a single unit test
        with_changes = test_my_model_simple_fixture_yml.replace("{string_c: ab}", "{string_c: bc}")
        write_config_file(with_changes, project.project_root, "models", "test_my_model.yml")
        results = run_dbt(
            ["test", "--select", "state:modified", "--state", "state"], expect_pass=False
        )
        assert len(results) == 1
        assert results[0].node.name.endswith("test_has_string_c_ab")

        # change underlying model logic
        write_config_file(
            test_my_model_simple_fixture_yml, project.project_root, "models", "test_my_model.yml"
        )
        write_file(
            my_model_vars_sql.replace("a+b as c,", "a + b as c,"),
            project.project_root,
            "models",
            "my_model.sql",
        )
        results = run_dbt(
            ["test", "--select", "state:modified", "--state", "state"], expect_pass=False
        )
        assert len(results) == 4


class TestUnitTestRetry(UnitTestState):
    def test_unit_test_retry(self, project):
        run_dbt(["run"])
        run_dbt(["test"], expect_pass=False)
        self.copy_state(project.project_root)

        results = run_dbt(["retry"], expect_pass=False)
        assert len(results) == 1


class TestUnitTestDeferState(UnitTestState):
    @pytest.fixture(scope="class")
    def other_schema(self, unique_schema):
        return unique_schema + "_other"

    @pytest.fixture(scope="class")
    def profiles_config_update(self, dbt_profile_target, unique_schema, other_schema):
        outputs = {"default": dbt_profile_target, "otherschema": deepcopy(dbt_profile_target)}
        outputs["default"]["schema"] = unique_schema
        outputs["otherschema"]["schema"] = other_schema
        return {"test": {"outputs": outputs, "target": "default"}}

    def test_unit_test_defer_state(self, project):
        run_dbt(["run", "--target", "otherschema"])
        self.copy_state(project.project_root)
        results = run_dbt(["test", "--defer", "--state", "state"], expect_pass=False)
        assert len(results) == 4
        assert sorted([r.status for r in results]) == ["fail", "pass", "pass", "pass"]
