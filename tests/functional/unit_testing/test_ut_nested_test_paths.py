import pytest

from dbt.tests.util import get_manifest, run_dbt

my_model_sql = """
select 1 as id
"""

my_model_expected_sql = """
select 1 as id
"""

real_singular_test_sql = """
select 1 as id where false
"""

unit_tests_yml = """
unit_tests:
  - name: test_my_model
    model: my_model
    given: []
    expect:
      format: sql
      fixture: my_model__expected
"""


# https://github.com/dbt-labs/dbt/issues/16507
class TestNestedTestPathsFixturesAreNotSingularTests:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"test-paths": ["tests", "tests/dbt"]}

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "foo": {
                "my_model.sql": my_model_sql,
                "unit_tests.yml": unit_tests_yml,
            }
        }

    @pytest.fixture(scope="class")
    def tests(self):
        return {
            "real_singular_test.sql": real_singular_test_sql,
            "dbt": {"fixtures": {"foo": {"my_model__expected.sql": my_model_expected_sql}}},
        }

    def test_fixture_in_nested_test_path_is_not_a_singular_test(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)

        test_node_ids = {node_id for node_id in manifest.nodes if node_id.startswith("test.")}
        assert test_node_ids == {"test.test.real_singular_test"}
        assert "unit_test.test.my_model.test_my_model" in manifest.unit_tests

        results = run_dbt(["ls", "--select", "foo", "--output", "name"])
        assert sorted(results) == ["my_model", "test_my_model"]


my_model_expected_csv = """id
1
"""

unit_tests_csv_yml = """
unit_tests:
  - name: test_my_model
    model: my_model
    given: []
    expect:
      format: csv
      fixture: my_model__expected
"""


class TestNestedTestPathsCsvFixture:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"test-paths": ["tests", "tests/dbt"]}

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "foo": {
                "my_model.sql": my_model_sql,
                "unit_tests.yml": unit_tests_csv_yml,
            }
        }

    @pytest.fixture(scope="class")
    def tests(self):
        return {
            "real_singular_test.sql": real_singular_test_sql,
            "dbt": {"fixtures": {"foo": {"my_model__expected.csv": my_model_expected_csv}}},
        }

    def test_csv_fixture_in_nested_test_path(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)

        test_node_ids = {node_id for node_id in manifest.nodes if node_id.startswith("test.")}
        assert test_node_ids == {"test.test.real_singular_test"}

        results = run_dbt(["build", "--select", "foo"])
        assert sorted(r.node.unique_id for r in results) == [
            "model.test.my_model",
            "unit_test.test.my_model.test_my_model",
        ]
