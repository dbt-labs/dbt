import json
from pathlib import Path

import pytest

from dbt.contracts.results import RunExecutionResult, NodeStatus
from dbt.tests.util import run_dbt

models__one_sql = """
select 1
"""

models__two_sql = """
-- depends_on: {{ ref('one') }}
select 1 /failed
"""


class FailFastBase:
    @pytest.fixture(scope="class")
    def models(self):
        return {"one.sql": models__one_sql, "two.sql": models__two_sql}


class TestFastFailingDuringRun(FailFastBase):
    def test_fail_fast_run(
        self,
        project,
        models,  # noqa: F811
    ):
        res = run_dbt(["run", "--fail-fast", "--threads", "1"], expect_pass=False)

        assert isinstance(res, RunExecutionResult)
        assert len(res.results) == 2
        assert res.results[0].status == NodeStatus.Success
        assert res.results[1].status == NodeStatus.Error

        run_results_file = Path(project.project_root) / "target/run_results.json"
        assert run_results_file.is_file()
        with run_results_file.open() as run_results_str:
            run_results = json.loads(run_results_str.read())
            assert len(run_results["results"]) == 2
            assert run_results["results"][0]["status"] == NodeStatus.Success
            assert run_results["results"][1]["status"] == NodeStatus.Error


class TestFailFastFromConfig(FailFastBase):
    @pytest.fixture(scope="class")
    def profiles_config_update(self):
        return {
            "config": {
                "send_anonymous_usage_stats": False,
                "fail_fast": True,
            }
        }

    def test_fail_fast_run_user_config(
        self,
        project,
        models,  # noqa: F811
    ):
        res = run_dbt(["run", "--threads", "1"], expect_pass=False)
        assert isinstance(res, RunExecutionResult)
        assert len(res.results) == 2
        assert res.results[0].status == NodeStatus.Success
        assert res.results[1].status == NodeStatus.Error
