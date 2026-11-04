import pytest

from dbt.exceptions import CompilationError
from dbt.tests.util import run_dbt

# Minimal repro for dbt-core #12975: relationships test with invalid to: ref() in arguments.

models__my_model_sql = "select 1 as pk_line_item"

models__valid_parent_sql = "select 1 as pk_line_item"

schema_yml = """
version: 2

models:
  - name: my_model
    columns:
      - name: fk_line_item
        data_tests:
          - not_null
          - relationships:
              arguments:
                field: pk_line_item
                to: ref('missing_parent_model')
"""


class TestInvalidRefInGenericTestArguments:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "my_model.sql": models__my_model_sql,
            "valid_parent.sql": models__valid_parent_sql,
            "schema.yml": schema_yml,
        }

    def test_parse_fails_on_missing_ref_in_generic_test(self, project):
        with pytest.raises(CompilationError) as excinfo:
            run_dbt(["parse"])

        assert "missing_parent_model" in str(excinfo.value)
        assert "which was not found" in str(excinfo.value)
