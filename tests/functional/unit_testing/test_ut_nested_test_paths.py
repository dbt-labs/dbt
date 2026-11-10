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
