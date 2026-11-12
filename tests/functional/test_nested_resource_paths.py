import pytest

from dbt.tests.util import get_manifest, rm_file, run_dbt, write_file

select_sql = """
select 1 as id
"""

singular_test_sql = """
select 1 as id where false
"""

updated_singular_test_sql = """
select 1 as id where 1 = 0
"""

is_positive_generic_test_sql = """
{% test is_positive(model, column_name) %}
select * from {{ model }} where {{ column_name }} <= 0
{% endtest %}
"""

is_positive_schema_yml = """
models:
  - name: my_model
    columns:
      - name: id
        data_tests:
          - is_positive
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


class TestNestedTestPathsGenericTest:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"test-paths": ["tests", "tests/dbt"]}

    @pytest.fixture(scope="class")
    def models(self):
        return {"my_model.sql": select_sql, "schema.yml": is_positive_schema_yml}

    @pytest.fixture(scope="class")
    def tests(self):
        return {"dbt": {"generic": {"is_positive.sql": is_positive_generic_test_sql}}}

    def test_nested_generic_test_is_not_a_singular_test(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)

        assert "test.test.is_positive" not in manifest.nodes
        assert "macro.test.test_is_positive" in manifest.macros
        test_node_ids = [node_id for node_id in manifest.nodes if node_id.startswith("test.")]
        assert len(test_node_ids) == 1
        assert test_node_ids[0].startswith("test.test.is_positive_my_model_id.")

        results = run_dbt(["build"])
        assert sorted(r.node.resource_type for r in results) == ["model", "test"]


class TestNestedTestPathsPartialParsing:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"test-paths": ["tests", "tests/dbt"]}

    @pytest.fixture(scope="class")
    def tests(self):
        return {"dbt": {"inner_test.sql": singular_test_sql}}

    def test_nested_singular_test_partial_parsing(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        assert manifest.nodes["test.test.inner_test"].fqn == ["test", "dbt", "inner_test"]

        # changed file keeps its single registration and fqn
        write_file(
            updated_singular_test_sql, project.project_root, "tests", "dbt", "inner_test.sql"
        )
        manifest = run_dbt(["--partial-parse", "parse"])
        test_node_ids = [node_id for node_id in manifest.nodes if node_id.startswith("test.")]
        assert test_node_ids == ["test.test.inner_test"]
        node = manifest.nodes["test.test.inner_test"]
        assert node.fqn == ["test", "dbt", "inner_test"]
        assert "1 = 0" in node.raw_code

        # added file is registered once, from the first configured test path
        write_file(singular_test_sql, project.project_root, "tests", "dbt", "added_test.sql")
        manifest = run_dbt(["--partial-parse", "parse"])
        assert manifest.nodes["test.test.added_test"].fqn == ["test", "dbt", "added_test"]

        # deleted file is removed
        rm_file(project.project_root, "tests", "dbt", "inner_test.sql")
        manifest = run_dbt(["--partial-parse", "parse"])
        test_node_ids = [node_id for node_id in manifest.nodes if node_id.startswith("test.")]
        assert test_node_ids == ["test.test.added_test"]
