import pytest
import yaml

from dbt.tests.util import run_dbt
from tests.functional.adapter.dbt_run_operations.fixtures import happy_macros_sql


# -- Below we define base classes for tests you import based on if your adapter supports dbt run-operation or not --
class BaseRunOperationResult:
    @pytest.fixture(scope="class")
    def macros(self):
        return {"happy_macros.sql": happy_macros_sql}

    def run_operation(self, macro, expect_pass=True, extra_args=None, **kwargs):
        args = ["run-operation", macro]
        if kwargs:
            args.extend(("--args", yaml.safe_dump(kwargs)))
        if extra_args:
            args.extend(extra_args)
        return run_dbt(args, expect_pass=expect_pass)

    def test_result_without_return(self, project):
        results = self.run_operation("select_something", name="world")
        assert results.results[0].agate_table is None

    def test_result_with_return(self, project):
        results = self.run_operation("select_something_with_return", name="world")
        assert len(results.results[0].agate_table) == 1
        assert results.results[0].agate_table.rows[0]["name"] == "hello, world"


class TestPostgresRunOperationResult(BaseRunOperationResult):
    pass
