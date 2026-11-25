import os

import pytest

from dbt.tests.util import run_dbt, write_file

os.environ["DBT_PP_TEST"] = "true"

# Resource names may contain dots ("a.b.orders"), so their unique_ids have more
# than three dot-separated parts. Partial parsing must still match those nodes
# to their schema file entries, and the result must equal a full parse.

where_macro_sql = """
{% macro dotted_names_where() %}{{ return('1 = 1') }}{% endmacro %}
"""

where_macro_edited_sql = """
{% macro dotted_names_where() %}{{ return('1 = 1') }}{% endmacro %}

{% macro dotted_names_unused() %}{{ return(1) }}{% endmacro %}
"""

has_rows_test_sql = """
{% test has_rows(model, flag='x') %}
select 1 from {{ model }} where {{ dotted_names_where() }} and '{{ flag }}' = '{{ flag }}'
having count(*) = 0
{% endtest %}
"""

model_sql = """
select 1 as id
"""

model_yml = """
models:
  - name: a.b.orders
    columns:
      - name: id
"""

model_edited_yml = """
models:
  - name: a.b.orders
    columns:
      - name: id
      - name: col2
"""

model_with_test_yml = """
models:
  - name: a.b.orders
    data_tests:
      - has_rows
"""

versioned_model_yml = """
models:
  - name: a.b.orders
    description: first
    latest_version: 2
    versions:
      - v: 1
      - v: 2
"""

versioned_model_edited_yml = """
models:
  - name: a.b.orders
    description: second
    latest_version: 2
    versions:
      - v: 1
      - v: 2
"""

prefix_models_yml = """
models:
  - name: a
    description: first
  - name: a.b.orders
    description: dotted
"""

prefix_models_edited_yml = """
models:
  - name: a
    description: second
  - name: a.b.orders
    description: dotted
"""

seed_csv = """id
1
"""

seed_with_test_yml = """
seeds:
  - name: a.b.colors
    data_tests:
      - has_rows
"""

analysis_sql = """
select 1 as id
"""

analysis_yml = """
analyses:
  - name: a.b.report
    description: first
"""

analysis_edited_yml = """
analyses:
  - name: a.b.report
    description: second
"""

singular_test_sql = """
select 1 as id where 1 = 0
"""

singular_test_yml = """
data_tests:
  - name: a.b.check
    description: first
"""

singular_test_edited_yml = """
data_tests:
  - name: a.b.check
    description: second
"""

model_with_env_var_test_argument_yml = """
models:
  - name: a.b.orders
    data_tests:
      - has_rows:
          arguments:
            flag: "{{ env_var('DBT_TEST_DOTTED_NAMES_FLAG', 'x') }}"
"""


def _node_state(node):
    return (
        node.patch_path,
        node.description,
        sorted(node.columns),
        sorted(getattr(node.depends_on, "nodes", [])),
        getattr(node.config, "where", None),
    )


def _parsed_state(manifest):
    return (
        {unique_id: _node_state(node) for unique_id, node in manifest.nodes.items()},
        {
            unique_id: [_node_state(node) for node in nodes]
            for unique_id, nodes in manifest.disabled.items()
        },
    )


def assert_matches_full_parse(partial_manifest):
    full_manifest = run_dbt(["--no-partial-parse", "parse"])
    assert _parsed_state(partial_manifest) == _parsed_state(full_manifest)


def _schema_file(manifest, path):
    return next(f for f in manifest.files.values() if f.path.original_file_path == path)


class TestDottedModelSchemaFileChange:
    @pytest.fixture(scope="class")
    def models(self):
        return {"a.b.orders.sql": model_sql, "schema.yml": model_yml}

    def test_dotted_model_schema_file_change(self, project):
        run_dbt(["parse"])
        write_file(model_edited_yml, project.project_root, "models", "schema.yml")

        manifest = run_dbt(["--partial-parse", "parse"])

        assert sorted(manifest.nodes["model.test.a.b.orders"].columns) == ["col2", "id"]
        assert_matches_full_parse(manifest)


class TestDottedVersionedModelSchemaFileChange:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "a.b.orders_v1.sql": model_sql,
            "a.b.orders_v2.sql": model_sql,
            "schema.yml": versioned_model_yml,
        }

    def test_dotted_versioned_model_schema_file_change(self, project):
        run_dbt(["parse"])
        write_file(versioned_model_edited_yml, project.project_root, "models", "schema.yml")

        manifest = run_dbt(["--partial-parse", "parse"])

        assert manifest.nodes["model.test.a.b.orders.v1"].description == "second"
        assert manifest.nodes["model.test.a.b.orders.v2"].description == "second"
        assert_matches_full_parse(manifest)


class TestDottedModelNamePrefixOfAnotherModel:
    @pytest.fixture(scope="class")
    def models(self):
        return {"a.sql": model_sql, "a.b.orders.sql": model_sql, "schema.yml": prefix_models_yml}

    def test_dotted_model_name_prefix_of_another_model(self, project):
        run_dbt(["parse"])
        # Only the entry for "a" changes; "a.b.orders" must keep its patch
        write_file(prefix_models_edited_yml, project.project_root, "models", "schema.yml")

        manifest = run_dbt(["--partial-parse", "parse"])

        assert manifest.nodes["model.test.a"].description == "second"
        assert manifest.nodes["model.test.a.b.orders"].description == "dotted"
        assert_matches_full_parse(manifest)


class TestDottedModelGenericTestMacroChange:
    @pytest.fixture(scope="class")
    def macros(self):
        return {"where.sql": where_macro_sql, "has_rows.sql": has_rows_test_sql}

    @pytest.fixture(scope="class")
    def models(self):
        return {"a.b.orders.sql": model_sql, "schema.yml": model_with_test_yml}

    def test_dotted_model_generic_test_macro_change(self, project):
        run_dbt(["parse"])
        # Changing a macro the generic test uses re-parses the test's schema file entry
        write_file(where_macro_edited_sql, project.project_root, "macros", "where.sql")

        manifest = run_dbt(["--partial-parse", "parse"])

        assert manifest.nodes["model.test.a.b.orders"].patch_path is not None
        assert_matches_full_parse(manifest)


class TestDottedSeedGenericTestMacroChange:
    @pytest.fixture(scope="class")
    def macros(self):
        return {"where.sql": where_macro_sql, "has_rows.sql": has_rows_test_sql}

    @pytest.fixture(scope="class")
    def seeds(self):
        return {"a.b.colors.csv": seed_csv, "schema.yml": seed_with_test_yml}

    def test_dotted_seed_generic_test_macro_change(self, project):
        run_dbt(["parse"])
        write_file(where_macro_edited_sql, project.project_root, "macros", "where.sql")

        manifest = run_dbt(["--partial-parse", "parse"])

        assert manifest.nodes["seed.test.a.b.colors"].patch_path is not None
        assert_matches_full_parse(manifest)


class TestDottedAnalysisSchemaFileChange:
    @pytest.fixture(scope="class")
    def analyses(self):
        return {"a.b.report.sql": analysis_sql, "schema.yml": analysis_yml}

    def test_dotted_analysis_schema_file_change(self, project):
        run_dbt(["parse"])
        write_file(analysis_edited_yml, project.project_root, "analyses", "schema.yml")

        manifest = run_dbt(["--partial-parse", "parse"])

        assert manifest.nodes["analysis.test.a.b.report"].description == "second"
        assert_matches_full_parse(manifest)


class TestDottedSingularTestSchemaFileChange:
    @pytest.fixture(scope="class")
    def tests(self):
        return {"a.b.check.sql": singular_test_sql, "schema.yml": singular_test_yml}

    def test_dotted_singular_test_schema_file_change(self, project):
        run_dbt(["parse"])
        write_file(singular_test_edited_yml, project.project_root, "tests", "schema.yml")

        manifest = run_dbt(["--partial-parse", "parse"])

        assert manifest.nodes["test.test.a.b.check"].description == "second"
        assert_matches_full_parse(manifest)


class TestDottedModelEnvVarInGenericTestArgument:
    @pytest.fixture(scope="class")
    def macros(self):
        return {"where.sql": where_macro_sql, "has_rows.sql": has_rows_test_sql}

    @pytest.fixture(scope="class")
    def models(self):
        return {"a.b.orders.sql": model_sql, "schema.yml": model_with_env_var_test_argument_yml}

    def test_dotted_model_env_var_in_generic_test_argument(self, project):
        os.environ["DBT_TEST_DOTTED_NAMES_FLAG"] = "one"
        try:
            # The test context records env vars used by a generic test under its
            # "<yaml key>.<resource name>"
            manifest = run_dbt(["parse"])
            schema_file = _schema_file(manifest, "models/schema.yml")
            assert schema_file.env_vars == {
                "models": {"a.b.orders": ["DBT_TEST_DOTTED_NAMES_FLAG"]}
            }

            # A changed env var re-parses the schema file entry
            os.environ["DBT_TEST_DOTTED_NAMES_FLAG"] = "two"
            manifest = run_dbt(["--partial-parse", "parse"])

            assert manifest.nodes["model.test.a.b.orders"].patch_path is not None
            assert_matches_full_parse(manifest)
        finally:
            del os.environ["DBT_TEST_DOTTED_NAMES_FLAG"]
