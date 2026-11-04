import pytest

from dbt.contracts.results import RunStatus, TestStatus
from dbt.tests.util import run_dbt, write_file

ephemeral_model_sql = """
{{ config(materialized="ephemeral") }}
select 1 as id, 'Emily' as first_name
"""

nested_ephemeral_model_sql = """
{{ config(materialized="ephemeral") }}
select * from {{ ref('ephemeral_model') }}
"""

customers_sql = """
select * from {{ ref('nested_ephemeral_model') }}
"""

test_sql_format_yml = """
unit_tests:
  - name: test_customers
    model: customers
    given:
      - input: ref('nested_ephemeral_model')
        format: sql
        rows: |
          select 1 as id, 'Emily' as first_name
    expect:
      rows:
        - {id: 1, first_name: Emily}
"""

failing_test_sql_format_yml = """
  - name: fail_test_customers
    model: customers
    given:
      - input: ref('nested_ephemeral_model')
        format: sql
        rows: |
          select 1 as id, 'Emily' as first_name
    expect:
      rows:
        - {id: 1, first_name: Joan}
"""


class TestUnitTestEphemeralInput:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "customers.sql": customers_sql,
            "ephemeral_model.sql": ephemeral_model_sql,
            "nested_ephemeral_model.sql": nested_ephemeral_model_sql,
            "tests.yml": test_sql_format_yml,
        }

    def test_ephemeral_input(self, project):
        results = run_dbt(["run"])
        len(results) == 1

        results = run_dbt(["test", "--select", "test_type:unit"])
        assert len(results) == 1

        results = run_dbt(["build"])
        assert len(results) == 2
        result_unique_ids = [result.node.unique_id for result in results]
        assert len(result_unique_ids) == 2
        assert "unit_test.test.customers.test_customers" in result_unique_ids

        # write failing unit test
        write_file(
            test_sql_format_yml + failing_test_sql_format_yml,
            project.project_root,
            "models",
            "tests.yml",
        )
        results = run_dbt(["build"], expect_pass=False)
        for result in results:
            if result.node.unique_id == "model.test.customers":
                assert result.status == RunStatus.Skipped
            elif result.node.unique_id == "unit_test.test.customers.fail_test_customers":
                assert result.status == TestStatus.Fail
        assert len(results) == 3


test_dict_format_yml = """
  - name: test_customers_dict_input
    model: customers
    given:
      - input: ref('nested_ephemeral_model')
        rows:
          - {id: 1, first_name: Emily}
    expect:
      rows:
        - {id: 1, first_name: Emily}
"""


class TestUnitTestEphemeralInputNonSqlFormat:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "customers.sql": customers_sql,
            "ephemeral_model.sql": ephemeral_model_sql,
            "nested_ephemeral_model.sql": nested_ephemeral_model_sql,
            "tests.yml": test_sql_format_yml + test_dict_format_yml,
        }

    def test_dict_input_for_ephemeral_model_errors_with_format_hint(self, project):
        results = run_dbt(["test", "--select", "test_type:unit"], expect_pass=False)
        assert len(results) == 2

        results_by_id = {result.node.unique_id: result for result in results}
        assert results_by_id["unit_test.test.customers.test_customers"].status == TestStatus.Pass
        dict_result = results_by_id["unit_test.test.customers.test_customers_dict_input"]
        assert dict_result.status == TestStatus.Error
        assert "format: sql" in dict_result.message
        assert "relation doesn't exist" not in dict_result.message
