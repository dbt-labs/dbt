import os
import json
import pytest
from datetime import timezone, datetime, timedelta
import yaml

import dbt.version
from dbt.artifacts.schemas.freshness import FreshnessResult
from dbt.artifacts.schemas.results import FreshnessStatus
from dbt.cli.main import dbtRunner
from tests.functional.sources.common_source_setup import BaseSourcesTest
from tests.functional.sources.fixtures import (
    error_models_schema_yml,
    error_models_model_sql,
    filtered_models_schema_yml,
    override_freshness_models_schema_yml,
    collect_freshness_macro_override_previous_return_signature,
    freshness_via_metadata_schema_yml,
)
from dbt.tests.util import AnyStringWith, AnyFloat


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

        # TODO: replace below calls - could they be more sane?
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


class TestSourceFreshness(SuccessfulSourceFreshnessTest):
    def test_source_freshness(self, project):
        # test_source.test_table should have a loaded_at field of `updated_at`
        # and a freshness of warn_after: 10 hours, error_after: 18 hours
        # by default, our data set is way out of date!

        results = self.run_dbt_with_vars(
            project, ["source", "freshness", "-o", "target/error_source.json"], expect_pass=False
        )
        assert len(results) == 1
        assert results[0].status == "error"
        self._assert_freshness_results("target/error_source.json", "error")

        self._set_updated_at_to(project, timedelta(hours=-12))
        results = self.run_dbt_with_vars(
            project,
            ["source", "freshness", "-o", "target/warn_source.json"],
        )
        assert len(results) == 1
        assert results[0].status == "warn"
        self._assert_freshness_results("target/warn_source.json", "warn")

        self._set_updated_at_to(project, timedelta(hours=-2))
        results = self.run_dbt_with_vars(
            project,
            ["source", "freshness", "-o", "target/pass_source.json"],
        )
        assert len(results) == 1
        assert results[0].status == "pass"
        self._assert_freshness_results("target/pass_source.json", "pass")


class TestSourceSnapshotFreshness(SuccessfulSourceFreshnessTest):
    def test_source_snapshot_freshness(self, project):
        """Ensures that the deprecated command `source snapshot-freshness`
        aliases to `source freshness` command.
        """
        results = self.run_dbt_with_vars(
            project,
            ["source", "snapshot-freshness", "-o", "target/error_source.json"],
            expect_pass=False,
        )
        assert len(results) == 1
        assert results[0].status == "error"
        self._assert_freshness_results("target/error_source.json", "error")

        self._set_updated_at_to(project, timedelta(hours=-12))
        results = self.run_dbt_with_vars(
            project,
            ["source", "snapshot-freshness", "-o", "target/warn_source.json"],
        )
        assert len(results) == 1
        assert results[0].status == "warn"
        self._assert_freshness_results("target/warn_source.json", "warn")

        self._set_updated_at_to(project, timedelta(hours=-2))
        results = self.run_dbt_with_vars(
            project,
            ["source", "snapshot-freshness", "-o", "target/pass_source.json"],
        )
        assert len(results) == 1
        assert results[0].status == "pass"
        self._assert_freshness_results("target/pass_source.json", "pass")


class TestSourceFreshnessSelection(SuccessfulSourceFreshnessTest):
    @pytest.fixture(scope="class")
    def project_config_update(self, logs_dir):
        return {
            "target-path": logs_dir,
        }

    def test_source_freshness_selection_select(self, project, logs_dir):
        """Tests node selection using the --select argument."""
        """Also validate that specify a target-path works as expected."""
        self._set_updated_at_to(project, timedelta(hours=-2))
        # select source directly
        results = self.run_dbt_with_vars(
            project,
            [
                "source",
                "freshness",
                "--select",
                "source:test_source.test_table",
            ],
        )
        assert len(results) == 1
        assert results[0].status == "pass"
        self._assert_freshness_results(f"{logs_dir}/sources.json", "pass")


class TestSourceFreshnessExclude(SuccessfulSourceFreshnessTest):
    def test_source_freshness_selection_exclude(self, project):
        """Tests node selection using the --select argument. It 'excludes' the
        only source in the project so it should return no results."""
        self._set_updated_at_to(project, timedelta(hours=-2))
        # exclude source directly
        results = self.run_dbt_with_vars(
            project,
            [
                "source",
                "freshness",
                "--exclude",
                "source:test_source.test_table",
                "-o",
                "target/exclude_source.json",
            ],
        )
        assert len(results) == 0


class TestSourceFreshnessGraph(SuccessfulSourceFreshnessTest):
    def test_source_freshness_selection_graph_operation(self, project):
        """Tests node selection using the --select argument with graph
        operations. `+descendant_model` == select all nodes `descendant_model`
        depends on.
        """
        self._set_updated_at_to(project, timedelta(hours=-2))
        # select model ancestors
        results = self.run_dbt_with_vars(
            project,
            [
                "source",
                "freshness",
                "--select",
                "+descendant_model",
                "-o",
                "target/ancestor_source.json",
            ],
        )
        assert len(results) == 1
        assert results[0].status == "pass"
        self._assert_freshness_results("target/ancestor_source.json", "pass")


class TestSourceFreshnessErrors(SuccessfulSourceFreshnessTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": error_models_schema_yml,
            "model.sql": error_models_model_sql,
        }

    def test_source_freshness_error(self, project):
        results = self.run_dbt_with_vars(project, ["source", "freshness"], expect_pass=False)
        assert len(results) == 1
        assert results[0].status == "runtime error"


class TestSourceFreshnessFilter(SuccessfulSourceFreshnessTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {"schema.yml": filtered_models_schema_yml}

    def test_source_freshness_all_records(self, project):
        # all records are filtered out
        self.run_dbt_with_vars(project, ["source", "freshness"], expect_pass=False)
        # we should insert a record with _id=101 that's fresh, but will still fail
        # because the filter excludes it
        self._set_updated_at_to(project, timedelta(hours=-2))
        self.run_dbt_with_vars(project, ["source", "freshness"], expect_pass=False)

        # we should now insert a record with _id=102 that's fresh, and the filter
        # includes it
        self._set_updated_at_to(project, timedelta(hours=-2))
        self.run_dbt_with_vars(project, ["source", "freshness"], expect_pass=True)


class TestOverrideSourceFreshness(SuccessfulSourceFreshnessTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {"schema.yml": override_freshness_models_schema_yml}

    @staticmethod
    def get_result_from_unique_id(data, unique_id):
        try:
            return list(filter(lambda x: x["unique_id"] == unique_id, data["results"]))[0]
        except IndexError:
            raise f"No result for the given unique_id. unique_id={unique_id}"

    def test_override_source_freshness(self, project):
        self._set_updated_at_to(project, timedelta(hours=-30))

        path = "target/pass_source.json"
        results = self.run_dbt_with_vars(
            project, ["source", "freshness", "-o", path], expect_pass=False
        )
        assert len(results) == 4  # freshness disabled for source_e

        assert os.path.exists(path)
        with open(path) as fp:
            data = json.load(fp)

        result_source_a = self.get_result_from_unique_id(data, "source.test.test_source.source_a")
        assert result_source_a["status"] == "error"

        expected = {
            "warn_after": {"count": 6, "period": "hour"},
            "error_after": {"count": 24, "period": "hour"},
            "filter": None,
        }
        assert result_source_a["criteria"] == expected

        result_source_b = self.get_result_from_unique_id(data, "source.test.test_source.source_b")
        assert result_source_b["status"] == "error"

        expected = {
            "warn_after": {"count": 6, "period": "hour"},
            "error_after": {"count": 24, "period": "hour"},
            "filter": None,
        }
        assert result_source_b["criteria"] == expected

        result_source_c = self.get_result_from_unique_id(data, "source.test.test_source.source_c")
        assert result_source_c["status"] == "warn"

        expected = {
            "warn_after": {"count": 6, "period": "hour"},
            "error_after": None,
            "filter": None,
        }
        assert result_source_c["criteria"] == expected

        result_source_d = self.get_result_from_unique_id(data, "source.test.test_source.source_d")
        assert result_source_d["status"] == "warn"

        expected = {
            "warn_after": {"count": 6, "period": "hour"},
            "error_after": {"count": 72, "period": "hour"},
            "filter": None,
        }
        assert result_source_d["criteria"] == expected


class TestSourceFreshnessMacroOverride(SuccessfulSourceFreshnessTest):
    @pytest.fixture(scope="class")
    def macros(self):
        return {
            "collect_freshness.sql": collect_freshness_macro_override_previous_return_signature
        }

    def test_source_freshness(self, project):
        # ensure that the deprecation warning is raised
        vars_dict = {
            "test_run_schema": project.test_schema,
            "test_loaded_at": project.adapter.quote("updated_at"),
        }
        events = []
        dbtRunner(callbacks=[events.append]).invoke(
            ["source", "freshness", "--vars", yaml.safe_dump(vars_dict)]
        )
        matches = list([e for e in events if e.info.name == "CollectFreshnessReturnSignature"])
        assert matches


class TestMetadataFreshnessFails:
    @pytest.fixture(scope="class")
    def models(self):
        return {"schema.yml": freshness_via_metadata_schema_yml}

    def test_metadata_freshness_unsupported_parse_warning(self, project):
        """Since the default test adapter (postgres) does not support metadata
        based source freshness checks, trying to use that mechanism should
        result in a parse-time warning."""
        got_warning = False

        def warning_probe(e):
            nonlocal got_warning
            if e.info.name == "FreshnessConfigProblem" and e.info.level == "warn":
                got_warning = True

        runner = dbtRunner(callbacks=[warning_probe])
        runner.invoke(["parse"])

        assert got_warning

    def test_metadata_freshness_unsupported_error_when_run(self, project):

        runner = dbtRunner()
        result = runner.invoke(["source", "freshness"])
        assert isinstance(result.result, FreshnessResult)
        assert len(result.result.results) == 1
        freshness_result = result.result.results[0]
        assert freshness_result.status == FreshnessStatus.RuntimeErr
        assert "Could not compute freshness for source test_table" in freshness_result.message


class TestHooksInSourceFreshness(SuccessfulSourceFreshnessTest):
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "config-version": 2,
            "on-run-start": ["{{ log('on-run-start hooks called') }}"],
            "on-run-end": ["{{ log('on-run-end hooks called') }}"],
            "flags": {
                "source_freshness_run_project_hooks": True,
            },
        }

    def test_hooks_do_run_for_source_freshness(
        self,
        project,
    ):
        _, log_output = self.run_dbt_and_capture_with_vars(
            project,
            [
                "source",
                "freshness",
            ],
            expect_pass=False,
        )
        assert "on-run-start" in log_output
        assert "on-run-end" in log_output


class TestHooksInSourceFreshnessError:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": error_models_schema_yml,
            "model.sql": error_models_model_sql,
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "config-version": 2,
            "on-run-start": ["select fake_column from table_does_not_exist"],
            "flags": {
                "source_freshness_run_project_hooks": True,
            },
        }

    def test_hooks_do_not_run_for_source_freshness(
        self,
        project,
    ):
        run_result_error = None

        def run_result_error_probe(e):
            nonlocal run_result_error
            if (
                e.info.name == "RunResultError"
                and e.info.level == "error"
                and "on-run-start" in e.info.msg
            ):
                run_result_error = e.info.msg

        runner = dbtRunner(callbacks=[run_result_error_probe])
        runner.invoke(["source", "freshness"])
        assert 'relation "table_does_not_exist" does not exist' in run_result_error


class TestHooksInSourceFreshnessDisabled(SuccessfulSourceFreshnessTest):
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "config-version": 2,
            "on-run-start": ["{{ log('on-run-start hooks called') }}"],
            "on-run-end": ["{{ log('on-run-end hooks called') }}"],
            "flags": {
                "source_freshness_run_project_hooks": False,
            },
        }

    def test_hooks_do_run_for_source_freshness(
        self,
        project,
    ):
        _, log_output = self.run_dbt_and_capture_with_vars(
            project,
            [
                "source",
                "freshness",
            ],
            expect_pass=False,
        )
        assert "on-run-start" not in log_output
        assert "on-run-end" not in log_output


class TestHooksInSourceFreshnessDefault(SuccessfulSourceFreshnessTest):
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "config-version": 2,
            "on-run-start": ["{{ log('on-run-start hooks called') }}"],
            "on-run-end": ["{{ log('on-run-end hooks called') }}"],
        }

    def test_hooks_do_run_for_source_freshness(
        self,
        project,
    ):
        _, log_output = self.run_dbt_and_capture_with_vars(
            project,
            [
                "source",
                "freshness",
            ],
            expect_pass=False,
        )
        # default behaviour - no hooks run in source freshness
        assert "on-run-start" not in log_output
        assert "on-run-end" not in log_output
