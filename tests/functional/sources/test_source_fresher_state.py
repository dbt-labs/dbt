import os
import json
import shutil
import pytest
from datetime import timezone, datetime, timedelta

from dbt_common.exceptions import DbtInternalError


from dbt.tests.util import AnyStringWith, AnyFloat
import dbt.version
from tests.functional.sources.common_source_setup import BaseSourcesTest

from tests.functional.sources.fixtures import (
    error_models_schema_yml,
    models_newly_added_model_sql,
    models_newly_added_error_model_sql,
)
from dbt.contracts.results import FreshnessExecutionResultArtifact


# TODO: We may create utility classes to handle reusable fixtures.
def copy_to_previous_state():
    shutil.copyfile("target/manifest.json", "previous_state/manifest.json")
    shutil.copyfile("target/run_results.json", "previous_state/run_results.json")


class SuccessfulSourceFreshnessTest(BaseSourcesTest):
    @pytest.fixture(scope="class", autouse=True)
    def setUp(self, project):
        self.run_dbt_with_vars(project, ["seed"])
        pytest._id = 101
        pytest.freshness_start_time = datetime.now(timezone.utc).replace(tzinfo=None)
        # this is the db initial value
        pytest.last_inserted_time = "2016-09-19T14:45:51+00:00"

        os.environ["DBT_ENV_CUSTOM_ENV_key"] = "value"

        yield

        del os.environ["DBT_ENV_CUSTOM_ENV_key"]

    def _set_updated_at_to(self, project, delta):
        insert_time = datetime.now(timezone.utc).replace(tzinfo=None) + delta
        timestr = insert_time.strftime("%Y-%m-%d %H:%M:%S")
        # favorite_color,id,first_name,email,ip_address,updated_at
        insert_id = pytest._id
        pytest._id += 1
        quoted_columns = ",".join(
            project.adapter.quote(c)
            for c in ("favorite_color", "id", "first_name", "email", "ip_address", "updated_at")
        )
        kwargs = {
            "schema": project.test_schema,
            "time": timestr,
            "id": insert_id,
            "source": project.adapter.quote("source"),
            "quoted_columns": quoted_columns,
        }
        raw_code = """INSERT INTO {schema}.{source}
            ({quoted_columns})
        VALUES (
            'blue',{id},'Jake','abc@example.com','192.168.1.1','{time}'
        )""".format(
            **kwargs
        )
        project.run_sql(raw_code)
        pytest.last_inserted_time = insert_time.strftime("%Y-%m-%dT%H:%M:%S+00:00")

    def assertBetween(self, timestr, start, end=None):
        datefmt = "%Y-%m-%dT%H:%M:%S.%fZ"
        if end is None:
            end = datetime.now(timezone.utc).replace(tzinfo=None)

        parsed = datetime.strptime(timestr, datefmt)

        assert start <= parsed
        assert end >= parsed

    def _assert_freshness_results(self, path, state):
        assert os.path.exists(path)
        with open(path) as fp:
            data = json.load(fp)

        try:
            FreshnessExecutionResultArtifact.validate(data)
        except Exception:
            raise pytest.fail("FreshnessExecutionResultArtifact did not validate")
        assert set(data) == {"metadata", "results", "elapsed_time"}
        assert "generated_at" in data["metadata"]
        assert isinstance(data["elapsed_time"], float)
        self.assertBetween(data["metadata"]["generated_at"], pytest.freshness_start_time)
        assert (
            data["metadata"]["dbt_schema_version"]
            == "https://schemas.getdbt.com/dbt/sources/v3.json"
        )
        assert data["metadata"]["dbt_version"] == dbt.version.__version__
        key = "key"
        if os.name == "nt":
            key = key.upper()
        assert data["metadata"]["env"] == {key: "value"}

        last_inserted_time = pytest.last_inserted_time

        assert len(data["results"]) == 1

        assert data["results"] == [
            {
                "unique_id": "source.test.test_source.test_table",
                "max_loaded_at": last_inserted_time,
                "snapshotted_at": AnyStringWith(),
                "max_loaded_at_time_ago_in_s": AnyFloat(),
                "status": state,
                "criteria": {
                    "filter": None,
                    "warn_after": {"count": 10, "period": "hour"},
                    "error_after": {"count": 18, "period": "hour"},
                },
                "adapter_response": {"_message": "SELECT 1", "code": "SELECT", "rows_affected": 1},
                "thread_id": AnyStringWith("Thread-"),
                "execution_time": AnyFloat(),
                "timing": [
                    {
                        "name": "compile",
                        "started_at": AnyStringWith(),
                        "completed_at": AnyStringWith(),
                    },
                    {
                        "name": "execute",
                        "started_at": AnyStringWith(),
                        "completed_at": AnyStringWith(),
                    },
                ],
            }
        ]


class TestSourceFresherNothingToDo(SuccessfulSourceFreshnessTest):
    def test_source_fresher_nothing_to_do(self, project):
        self.run_dbt_with_vars(project, ["run"])
        self._set_updated_at_to(project, timedelta(hours=-2))
        previous_state_results = self.run_dbt_with_vars(
            project, ["source", "freshness", "-o", "previous_state/sources.json"]
        )
        self._assert_freshness_results("previous_state/sources.json", "pass")
        copy_to_previous_state()

        current_state_results = self.run_dbt_with_vars(
            project, ["source", "freshness", "-o", "target/sources.json"]
        )
        self._assert_freshness_results("target/sources.json", "pass")

        assert previous_state_results[0].max_loaded_at == current_state_results[0].max_loaded_at

        source_fresher_results = self.run_dbt_with_vars(
            project,
            ["test", "-s", "source_status:fresher+", "--defer", "--state", "./previous_state"],
        )
        assert source_fresher_results.results == []


class TestSourceFresherRun(SuccessfulSourceFreshnessTest):
    def test_source_fresher_run_error(self, project):
        self.run_dbt_with_vars(project, ["run"])
        previous_state_results = self.run_dbt_with_vars(
            project,
            ["source", "freshness", "-o", "previous_state/sources.json"],
            expect_pass=False,
        )
        self._assert_freshness_results("previous_state/sources.json", "error")
        copy_to_previous_state()

        self._set_updated_at_to(project, timedelta(hours=-20))
        current_state_results = self.run_dbt_with_vars(
            project,
            ["source", "freshness", "-o", "target/sources.json"],
            expect_pass=False,
        )
        self._assert_freshness_results("target/sources.json", "error")

        assert previous_state_results[0].max_loaded_at < current_state_results[0].max_loaded_at

        source_fresher_results = self.run_dbt_with_vars(
            project,
            ["run", "-s", "source_status:fresher", "--defer", "--state", "previous_state"],
        )
        assert source_fresher_results.results == []

        source_fresher_plus_results = self.run_dbt_with_vars(
            project,
            ["run", "-s", "source_status:fresher+", "--defer", "--state", "previous_state"],
        )
        nodes = set([elem.node.name for elem in source_fresher_plus_results])
        assert nodes == {"descendant_model"}

    def test_source_fresher_run_warn(self, project):
        self.run_dbt_with_vars(project, ["run"])
        self._set_updated_at_to(project, timedelta(hours=-17))
        previous_state_results = self.run_dbt_with_vars(
            project,
            ["source", "freshness", "-o", "previous_state/sources.json"],
            expect_pass=True,
        )
        self._assert_freshness_results("previous_state/sources.json", "warn")
        copy_to_previous_state()

        self._set_updated_at_to(project, timedelta(hours=-11))
        current_state_results = self.run_dbt_with_vars(
            project, ["source", "freshness", "-o", "target/sources.json"]
        )
        self._assert_freshness_results("target/sources.json", "warn")

        assert previous_state_results[0].max_loaded_at < current_state_results[0].max_loaded_at

        source_fresher_results = self.run_dbt_with_vars(
            project,
            ["run", "-s", "source_status:fresher", "--defer", "--state", "previous_state"],
        )
        assert source_fresher_results.results == []

        source_fresher_plus_results = self.run_dbt_with_vars(
            project,
            ["run", "-s", "source_status:fresher+", "--defer", "--state", "previous_state"],
        )
        nodes = set([elem.node.name for elem in source_fresher_plus_results])
        assert nodes == {"descendant_model"}

    def test_source_fresher_run_pass(self, project):
        self.run_dbt_with_vars(project, ["run"])
        self._set_updated_at_to(project, timedelta(hours=-2))
        previous_state_results = self.run_dbt_with_vars(
            project, ["source", "freshness", "-o", "previous_state/sources.json"]
        )
        self._assert_freshness_results("previous_state/sources.json", "pass")
        copy_to_previous_state()

        self._set_updated_at_to(project, timedelta(hours=-1))
        current_state_results = self.run_dbt_with_vars(
            project, ["source", "freshness", "-o", "target/sources.json"]
        )
        self._assert_freshness_results("target/sources.json", "pass")

        assert previous_state_results[0].max_loaded_at < current_state_results[0].max_loaded_at

        source_fresher_results = self.run_dbt_with_vars(
            project,
            ["run", "-s", "source_status:fresher", "--defer", "--state", "previous_state"],
        )
        assert source_fresher_results.results == []

        source_fresher_plus_results = self.run_dbt_with_vars(
            project,
            ["run", "-s", "source_status:fresher+", "--defer", "--state", "previous_state"],
        )
        nodes = set([elem.node.name for elem in source_fresher_plus_results])
        assert nodes == {"descendant_model"}


class TestSourceFresherBuildStateModified(SuccessfulSourceFreshnessTest):
    def test_source_fresher_build_state_modified_pass(self, project, project_root):
        self.run_dbt_with_vars(project, ["run"])

        self._set_updated_at_to(project, timedelta(hours=-2))
        previous_state_results = self.run_dbt_with_vars(
            project, ["source", "freshness", "-o", "previous_state/sources.json"]
        )
        self._assert_freshness_results("previous_state/sources.json", "pass")

        self._set_updated_at_to(project, timedelta(hours=-1))
        current_state_results = self.run_dbt_with_vars(
            project, ["source", "freshness", "-o", "target/sources.json"]
        )
        self._assert_freshness_results("target/sources.json", "pass")

        assert previous_state_results[0].max_loaded_at < current_state_results[0].max_loaded_at

        models_path = project_root.join("models/")
        assert os.path.exists(models_path)
        with open(f"{models_path}/newly_added_model.sql", "w") as fp:
            fp.write(models_newly_added_model_sql)

        copy_to_previous_state()
        state_modified_results = self.run_dbt_with_vars(
            project,
            [
                "build",
                "--select",
                "source_status:fresher+",
                "state:modified+",
                "--defer",
                "--state",
                "previous_state",
            ],
        )
        nodes = set([elem.node.name for elem in state_modified_results])
        assert nodes == {
            "newly_added_model",
            "source_unique_test_source_test_table_id",
            "unique_descendant_model_id",
            "not_null_descendant_model_id",
            "source_not_null_test_source_test_table_id",
            "descendant_model",
            "source_relationships_test_source_test_table_favorite_color__favorite_color__ref_descendant_model_",
            "relationships_descendant_model_favorite_color__favorite_color__source_test_source_test_table_",
        }


class TestSourceFresherRuntimeError(SuccessfulSourceFreshnessTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": error_models_schema_yml,
        }

    def test_runtime_error_states(self, project):
        self.run_dbt_with_vars(project, ["run"])
        previous_state_results = self.run_dbt_with_vars(
            project,
            ["source", "freshness", "-o", "previous_state/sources.json"],
            expect_pass=False,
        )
        assert len(previous_state_results) == 1
        assert previous_state_results[0].status == "runtime error"
        copy_to_previous_state()

        self._set_updated_at_to(project, timedelta(hours=-1))
        current_state_results = self.run_dbt_with_vars(
            project,
            ["source", "freshness", "-o", "target/sources.json"],
            expect_pass=False,
        )
        assert len(current_state_results) == 1
        assert current_state_results[0].status == "runtime error"

        assert not hasattr(previous_state_results[0], "max_loaded_at")
        assert not hasattr(current_state_results[0], "max_loaded_at")

        source_fresher_results = self.run_dbt_with_vars(
            project,
            ["test", "-s", "source_status:fresher", "--defer", "--state", "previous_state"],
        )
        assert source_fresher_results.results == []


class TestSourceFresherTest(SuccessfulSourceFreshnessTest):
    def test_source_fresher_run_error(self, project):
        self.run_dbt_with_vars(project, ["run"])
        previous_state_results = self.run_dbt_with_vars(
            project,
            ["source", "freshness", "-o", "previous_state/sources.json"],
            expect_pass=False,
        )
        self._assert_freshness_results("previous_state/sources.json", "error")
        copy_to_previous_state()

        self._set_updated_at_to(project, timedelta(hours=-20))
        current_state_results = self.run_dbt_with_vars(
            project,
            ["source", "freshness", "-o", "target/sources.json"],
            expect_pass=False,
        )
        self._assert_freshness_results("target/sources.json", "error")

        assert previous_state_results[0].max_loaded_at < current_state_results[0].max_loaded_at

        source_fresher_results = self.run_dbt_with_vars(
            project,
            ["test", "-s", "source_status:fresher", "--defer", "--state", "previous_state"],
        )
        nodes = set([elem.node.name for elem in source_fresher_results])
        assert nodes == {
            "relationships_descendant_model_favorite_color__favorite_color__source_test_source_test_table_",
            "source_not_null_test_source_test_table_id",
            "source_relationships_test_source_test_table_favorite_color__favorite_color__ref_descendant_model_",
            "source_unique_test_source_test_table_id",
        }

        source_fresher_plus_results = self.run_dbt_with_vars(
            project,
            ["test", "-s", "source_status:fresher+", "--defer", "--state", "previous_state"],
        )
        nodes = set([elem.node.name for elem in source_fresher_plus_results])
        assert nodes == {
            "relationships_descendant_model_favorite_color__favorite_color__source_test_source_test_table_",
            "source_not_null_test_source_test_table_id",
            "source_relationships_test_source_test_table_favorite_color__favorite_color__ref_descendant_model_",
            "source_unique_test_source_test_table_id",
            "unique_descendant_model_id",
            "not_null_descendant_model_id",
        }

    def test_source_fresher_test_warn(self, project):
        self.run_dbt_with_vars(project, ["run"])
        self._set_updated_at_to(project, timedelta(hours=-17))
        previous_state_results = self.run_dbt_with_vars(
            project,
            ["source", "freshness", "-o", "previous_state/sources.json"],
            expect_pass=True,
        )
        self._assert_freshness_results("previous_state/sources.json", "warn")
        copy_to_previous_state()

        self._set_updated_at_to(project, timedelta(hours=-11))
        current_state_results = self.run_dbt_with_vars(
            project,
            ["source", "freshness", "-o", "target/sources.json"],
            expect_pass=True,
        )
        self._assert_freshness_results("target/sources.json", "warn")

        assert previous_state_results[0].max_loaded_at < current_state_results[0].max_loaded_at

        source_fresher_results = self.run_dbt_with_vars(
            project,
            ["test", "-s", "source_status:fresher", "--defer", "--state", "previous_state"],
        )
        nodes = set([elem.node.name for elem in source_fresher_results])
        assert nodes == {
            "relationships_descendant_model_favorite_color__favorite_color__source_test_source_test_table_",
            "source_not_null_test_source_test_table_id",
            "source_relationships_test_source_test_table_favorite_color__favorite_color__ref_descendant_model_",
            "source_unique_test_source_test_table_id",
        }

        source_fresher_plus_results = self.run_dbt_with_vars(
            project,
            ["test", "-s", "source_status:fresher+", "--defer", "--state", "previous_state"],
        )
        nodes = set([elem.node.name for elem in source_fresher_plus_results])
        assert nodes == {
            "relationships_descendant_model_favorite_color__favorite_color__source_test_source_test_table_",
            "source_not_null_test_source_test_table_id",
            "source_relationships_test_source_test_table_favorite_color__favorite_color__ref_descendant_model_",
            "source_unique_test_source_test_table_id",
            "unique_descendant_model_id",
            "not_null_descendant_model_id",
        }

    def test_source_fresher_test_pass(self, project):
        self.run_dbt_with_vars(project, ["run"])
        self._set_updated_at_to(project, timedelta(hours=-2))
        previous_state_results = self.run_dbt_with_vars(
            project, ["source", "freshness", "-o", "previous_state/sources.json"]
        )
        self._assert_freshness_results("previous_state/sources.json", "pass")
        copy_to_previous_state()

        self._set_updated_at_to(project, timedelta(hours=-1))
        current_state_results = self.run_dbt_with_vars(
            project, ["source", "freshness", "-o", "target/sources.json"]
        )
        self._assert_freshness_results("target/sources.json", "pass")

        assert previous_state_results[0].max_loaded_at < current_state_results[0].max_loaded_at

        source_fresher_results = self.run_dbt_with_vars(
            project,
            ["test", "-s", "source_status:fresher", "--defer", "--state", "previous_state"],
        )
        nodes = set([elem.node.name for elem in source_fresher_results])
        assert nodes == {
            "relationships_descendant_model_favorite_color__favorite_color__source_test_source_test_table_",
            "source_not_null_test_source_test_table_id",
            "source_relationships_test_source_test_table_favorite_color__favorite_color__ref_descendant_model_",
            "source_unique_test_source_test_table_id",
        }

        source_fresher_plus_results = self.run_dbt_with_vars(
            project,
            ["test", "-s", "source_status:fresher+", "--defer", "--state", "previous_state"],
        )
        nodes = set([elem.node.name for elem in source_fresher_plus_results])
        assert nodes == {
            "relationships_descendant_model_favorite_color__favorite_color__source_test_source_test_table_",
            "source_not_null_test_source_test_table_id",
            "source_relationships_test_source_test_table_favorite_color__favorite_color__ref_descendant_model_",
            "source_unique_test_source_test_table_id",
            "unique_descendant_model_id",
            "not_null_descendant_model_id",
        }


class TestSourceFresherBuild(SuccessfulSourceFreshnessTest):
    def test_source_fresher_build_error(self, project):
        self.run_dbt_with_vars(project, ["build"])
        previous_state_results = self.run_dbt_with_vars(
            project,
            ["source", "freshness", "-o", "previous_state/sources.json"],
            expect_pass=False,
        )
        self._assert_freshness_results("previous_state/sources.json", "error")
        copy_to_previous_state()

        self._set_updated_at_to(project, timedelta(hours=-20))
        current_state_results = self.run_dbt_with_vars(
            project,
            ["source", "freshness", "-o", "target/sources.json"],
            expect_pass=False,
        )
        self._assert_freshness_results("target/sources.json", "error")

        assert previous_state_results[0].max_loaded_at < current_state_results[0].max_loaded_at

        source_fresher_results = self.run_dbt_with_vars(
            project,
            ["build", "-s", "source_status:fresher", "--defer", "--state", "previous_state"],
        )
        nodes = set([elem.node.name for elem in source_fresher_results])
        assert nodes == {
            "relationships_descendant_model_favorite_color__favorite_color__source_test_source_test_table_",
            "source_not_null_test_source_test_table_id",
            "source_relationships_test_source_test_table_favorite_color__favorite_color__ref_descendant_model_",
            "source_unique_test_source_test_table_id",
        }

        source_fresher_plus_results = self.run_dbt_with_vars(
            project,
            ["build", "-s", "source_status:fresher+", "--defer", "--state", "previous_state"],
        )
        nodes = set([elem.node.name for elem in source_fresher_plus_results])
        assert nodes == {
            "descendant_model",
            "relationships_descendant_model_favorite_color__favorite_color__source_test_source_test_table_",
            "source_not_null_test_source_test_table_id",
            "source_relationships_test_source_test_table_favorite_color__favorite_color__ref_descendant_model_",
            "source_unique_test_source_test_table_id",
            "unique_descendant_model_id",
            "not_null_descendant_model_id",
        }

    def test_source_fresher_build_warn(self, project):
        self.run_dbt_with_vars(project, ["build"])
        self._set_updated_at_to(project, timedelta(hours=-17))
        previous_state_results = self.run_dbt_with_vars(
            project,
            ["source", "freshness", "-o", "previous_state/sources.json"],
            expect_pass=True,
        )
        self._assert_freshness_results("previous_state/sources.json", "warn")
        copy_to_previous_state()

        self._set_updated_at_to(project, timedelta(hours=-11))
        current_state_results = self.run_dbt_with_vars(
            project, ["source", "freshness", "-o", "target/sources.json"]
        )
        self._assert_freshness_results("target/sources.json", "warn")

        assert previous_state_results[0].max_loaded_at < current_state_results[0].max_loaded_at

        source_fresher_results = self.run_dbt_with_vars(
            project,
            ["build", "-s", "source_status:fresher", "--defer", "--state", "previous_state"],
        )
        nodes = set([elem.node.name for elem in source_fresher_results])
        assert nodes == {
            "relationships_descendant_model_favorite_color__favorite_color__source_test_source_test_table_",
            "source_not_null_test_source_test_table_id",
            "source_relationships_test_source_test_table_favorite_color__favorite_color__ref_descendant_model_",
            "source_unique_test_source_test_table_id",
        }

        source_fresher_plus_results = self.run_dbt_with_vars(
            project,
            ["build", "-s", "source_status:fresher+", "--defer", "--state", "previous_state"],
        )
        nodes = set([elem.node.name for elem in source_fresher_plus_results])
        assert nodes == {
            "descendant_model",
            "relationships_descendant_model_favorite_color__favorite_color__source_test_source_test_table_",
            "source_not_null_test_source_test_table_id",
            "source_relationships_test_source_test_table_favorite_color__favorite_color__ref_descendant_model_",
            "source_unique_test_source_test_table_id",
            "unique_descendant_model_id",
            "not_null_descendant_model_id",
        }

    def test_source_fresher_build_pass(self, project):
        self.run_dbt_with_vars(project, ["build"])
        self._set_updated_at_to(project, timedelta(hours=-2))
        previous_state_results = self.run_dbt_with_vars(
            project, ["source", "freshness", "-o", "previous_state/sources.json"]
        )
        self._assert_freshness_results("previous_state/sources.json", "pass")
        copy_to_previous_state()

        self._set_updated_at_to(project, timedelta(hours=-1))
        current_state_results = self.run_dbt_with_vars(
            project, ["source", "freshness", "-o", "target/sources.json"]
        )
        self._assert_freshness_results("target/sources.json", "pass")

        assert previous_state_results[0].max_loaded_at < current_state_results[0].max_loaded_at

        source_fresher_results = self.run_dbt_with_vars(
            project,
            ["build", "-s", "source_status:fresher", "--defer", "--state", "previous_state"],
        )
        nodes = set([elem.node.name for elem in source_fresher_results])
        assert nodes == {
            "relationships_descendant_model_favorite_color__favorite_color__source_test_source_test_table_",
            "source_not_null_test_source_test_table_id",
            "source_relationships_test_source_test_table_favorite_color__favorite_color__ref_descendant_model_",
            "source_unique_test_source_test_table_id",
        }

        source_fresher_plus_results = self.run_dbt_with_vars(
            project,
            ["build", "-s", "source_status:fresher+", "--defer", "--state", "previous_state"],
        )
        nodes = set([elem.node.name for elem in source_fresher_plus_results])
        assert nodes == {
            "descendant_model",
            "relationships_descendant_model_favorite_color__favorite_color__source_test_source_test_table_",
            "source_not_null_test_source_test_table_id",
            "source_relationships_test_source_test_table_favorite_color__favorite_color__ref_descendant_model_",
            "source_unique_test_source_test_table_id",
            "unique_descendant_model_id",
            "not_null_descendant_model_id",
        }


class TestSourceFresherNoPreviousState(SuccessfulSourceFreshnessTest):
    def test_intentional_failure_no_previous_state(self, project):
        self.run_dbt_with_vars(project, ["run"])
        # TODO add the current and previous but with previous as null
        with pytest.raises(DbtInternalError) as excinfo:
            self.run_dbt_with_vars(
                project,
                ["run", "-s", "source_status:fresher", "--defer", "--state", "previous_state"],
            )
        assert "No previous state comparison freshness results in sources.json" in str(
            excinfo.value
        )


class TestSourceFresherNoCurrentState(SuccessfulSourceFreshnessTest):
    def test_intentional_failure_no_previous_state(self, project):
        self.run_dbt_with_vars(project, ["run"])
        previous_state_results = self.run_dbt_with_vars(
            project,
            ["source", "freshness", "-o", "previous_state/sources.json"],
            expect_pass=False,
        )
        self._assert_freshness_results("previous_state/sources.json", "error")
        copy_to_previous_state()
        assert previous_state_results[0].max_loaded_at is not None

        with pytest.raises(DbtInternalError) as excinfo:
            self.run_dbt_with_vars(
                project,
                ["run", "-s", "source_status:fresher", "--defer", "--state", "previous_state"],
            )
        assert "No current state comparison freshness results in sources.json" in str(
            excinfo.value
        )


class TestSourceFresherBuildResultSelectors(SuccessfulSourceFreshnessTest):
    def test_source_fresher_build_state_modified_pass(self, project, project_root):
        models_path = project_root.join("models/")
        assert os.path.exists(models_path)
        with open(f"{models_path}/newly_added_error_model.sql", "w") as fp:
            fp.write(models_newly_added_error_model_sql)

        self.run_dbt_with_vars(project, ["run"], expect_pass=False)

        self._set_updated_at_to(project, timedelta(hours=-2))
        previous_state_results = self.run_dbt_with_vars(
            project, ["source", "freshness", "-o", "previous_state/sources.json"]
        )

        self._assert_freshness_results("previous_state/sources.json", "pass")
        copy_to_previous_state()

        self._set_updated_at_to(project, timedelta(hours=-1))
        current_state_results = self.run_dbt_with_vars(
            project, ["source", "freshness", "-o", "target/sources.json"]
        )

        self._assert_freshness_results("target/sources.json", "pass")

        assert previous_state_results[0].max_loaded_at < current_state_results[0].max_loaded_at

        state_modified_results = self.run_dbt_with_vars(
            project,
            [
                "build",
                "--select",
                "source_status:fresher+",
                "result:error+",
                "--defer",
                "--state",
                "previous_state",
            ],
            expect_pass=False,
        )
        nodes = set([elem.node.name for elem in state_modified_results])
        assert nodes == {
            "newly_added_error_model",
            "source_unique_test_source_test_table_id",
            "unique_descendant_model_id",
            "not_null_descendant_model_id",
            "source_not_null_test_source_test_table_id",
            "descendant_model",
            "source_relationships_test_source_test_table_favorite_color__favorite_color__ref_descendant_model_",
            "relationships_descendant_model_favorite_color__favorite_color__source_test_source_test_table_",
        }
