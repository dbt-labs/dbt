import pytest

from dbt.tests.util import get_manifest, run_dbt

select_sql = """
select 1 as id
"""

singular_test_sql = """
select 1 as id where false
"""


# https://github.com/dbt-labs/dbt/issues/16507
# A file under nested resource paths is registered once, from the first configured
# path that contains it, which is also how dbt v2 builds the fqn.
class TestNestedTestPathsOuterFirst:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"test-paths": ["tests", "tests/dbt"]}

    @pytest.fixture(scope="class")
    def tests(self):
        return {"dbt": {"inner_test.sql": singular_test_sql}}

    def test_nested_singular_test_fqn(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)

        assert manifest.nodes["test.test.inner_test"].fqn == ["test", "dbt", "inner_test"]


class TestNestedTestPathsInnerFirst:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"test-paths": ["tests/dbt", "tests"]}

    @pytest.fixture(scope="class")
    def tests(self):
        return {"dbt": {"inner_test.sql": singular_test_sql}}

    def test_nested_singular_test_fqn(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)

        assert manifest.nodes["test.test.inner_test"].fqn == ["test", "inner_test"]


class TestNestedModelPaths:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"model-paths": ["models", "models/staging"]}

    @pytest.fixture(scope="class")
    def models(self):
        return {"staging": {"my_model.sql": select_sql}}

    def test_nested_model_fqn(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)

        assert manifest.nodes["model.test.my_model"].fqn == ["test", "staging", "my_model"]
