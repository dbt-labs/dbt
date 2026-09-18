from typing import Any, Dict, Union

import pytest

from dbt.cli.main import dbtRunner, dbtRunnerResult
from dbt.events.types import (
    DeprecatedModel,
    MainEncounteredError,
    MicrobatchModelNoEventTimeInputs,
)
from dbt.flags import get_flags
from dbt.tests.util import run_dbt, update_config_file
from dbt_common.events.base_types import EventLevel
from dbt_common.events.event_catcher import EventCatcher

ModelsDictSpec = Dict[str, Union[str, "ModelsDictSpec"]]

my_model_sql = """SELECT 1 AS id, 'cats are cute' AS description"""
schema_yml = """
version: 2
models:
  - name: my_model
    deprecation_date: 2020-01-01
"""


class BaseTestWarnErrorOptions:
    @pytest.fixture(scope="class")
    def models(self) -> ModelsDictSpec:
        return {"my_model.sql": my_model_sql, "schema.yml": schema_yml}

    @pytest.fixture(scope="function")
    def catcher(self) -> EventCatcher:
        return EventCatcher(event_to_catch=DeprecatedModel)

    @pytest.fixture(scope="function")
    def runner(self, catcher: EventCatcher) -> dbtRunner:
        return dbtRunner(callbacks=[catcher.catch])

    def assert_deprecation_warning(self, result: dbtRunnerResult, catcher: EventCatcher) -> None:
        assert result.success
        assert result.exception is None
        assert len(catcher.caught_events) == 1
        assert catcher.caught_events[0].info.level == EventLevel.WARN.value

    def assert_deprecation_error(self, result: dbtRunnerResult) -> None:
        assert not result.success
        assert result.exception is not None
        assert "Model my_model has passed its deprecation date of" in str(result.exception)


class TestWarnErrorOptionsFromCLICanSilence(BaseTestWarnErrorOptions):
    def test_can_silence(self, project, catcher: EventCatcher, runner: dbtRunner) -> None:
        result = runner.invoke(["run"])
        self.assert_deprecation_warning(result, catcher)

        catcher.flush()
        result = runner.invoke(["run", "--warn-error-options", "{'silence': ['DeprecatedModel']}"])
        assert result.success
        assert len(catcher.caught_events) == 0


class TestWarnErrorOptionsFromCLICanRaiseWarningToError(BaseTestWarnErrorOptions):
    def test_can_raise_warning_to_error(
        self, project, catcher: EventCatcher, runner: dbtRunner
    ) -> None:
        result = runner.invoke(["run"])
        self.assert_deprecation_warning(result, catcher)

        catcher.flush()
        result = runner.invoke(["run", "--warn-error-options", "{'include': ['DeprecatedModel']}"])
        self.assert_deprecation_error(result)

        catcher.flush()
        result = runner.invoke(
            [
                "run",
                "--warn-error-options",
                "{'include': 'all', 'warn': ['DeprecationsSummary', 'WEOIncludeExcludeDeprecation']}",
            ]
        )
        self.assert_deprecation_error(result)

        catcher.flush()
        result = runner.invoke(["run", "--warn-error-options", "{'error': ['DeprecatedModel']}"])
        self.assert_deprecation_error(result)

        catcher.flush()
        result = runner.invoke(
            ["run", "--warn-error-options", "{'error': 'all', 'warn': ['DeprecationsSummary']}"]
        )
        self.assert_deprecation_error(result)


class TestWarnErrorOptionsFromCLICanExcludeSpecificEvent(BaseTestWarnErrorOptions):
    def test_can_exclude_specific_event(
        self, project, catcher: EventCatcher, runner: dbtRunner
    ) -> None:
        result = runner.invoke(
            ["run", "--warn-error-options", "{'error': 'all', 'warn': ['DeprecationsSummary']}"]
        )
        self.assert_deprecation_error(result)

        catcher.flush()
        result = runner.invoke(
            [
                "run",
                "--warn-error-options",
                "{'error': 'all', 'exclude': ['DeprecatedModel', 'WEOIncludeExcludeDeprecation', 'DeprecationsSummary']}",
            ]
        )
        self.assert_deprecation_warning(result, catcher)

        catcher.flush()
        result = runner.invoke(
            [
                "run",
                "--warn-error-options",
                "{'error': 'all', 'warn': ['DeprecatedModel', 'DeprecationsSummary']}",
            ]
        )
        self.assert_deprecation_warning(result, catcher)


class TestWarnErrorOptionsFromCLICantSetBothIncludeAndError(BaseTestWarnErrorOptions):
    def test_cant_set_both_include_and_error(self, project, runner: dbtRunner) -> None:
        result = runner.invoke(
            ["run", "--warn-error-options", "{'include': 'all', 'error': 'all'}"]
        )
        assert not result.success
        assert result.exception is not None
        assert "Only `include` or `error` can be specified" in str(result.exception)

    def test_cant_set_both_exclude_and_warn(self, project, runner: dbtRunner) -> None:
        result = runner.invoke(
            [
                "run",
                "--warn-error-options",
                "{'include': 'all', 'exclude': ['DeprecatedModel'], 'warn': ['DeprecatedModel']}",
            ]
        )
        assert not result.success
        assert result.exception is not None
        assert "Only `exclude` or `warn` can be specified" in str(result.exception)


class BaseTestWarnErrorOptionsFromProject(BaseTestWarnErrorOptions):
    @pytest.fixture(scope="function")
    def clear_project_flags(self, project_root) -> None:
        # TODO: Is this still necessary now that the project based tests are broken into separate test classes?
        flags: Dict[str, Any] = {"flags": {}}
        update_config_file(flags, project_root, "dbt_project.yml")


class TestWarnErrorOptionsFromProjectCanSilence(BaseTestWarnErrorOptionsFromProject):
    def test_can_silence(
        self, project, clear_project_flags, project_root, catcher: EventCatcher, runner: dbtRunner
    ) -> None:
        result = runner.invoke(["run"])
        self.assert_deprecation_warning(result, catcher)

        silence_options = {"flags": {"warn_error_options": {"silence": ["DeprecatedModel"]}}}
        update_config_file(silence_options, project_root, "dbt_project.yml")

        catcher.flush()
        result = runner.invoke(["run"])
        assert result.success
        assert len(catcher.caught_events) == 0


class TestWarnErrorOptionsFromProjectCanRaiseWarningToError(BaseTestWarnErrorOptionsFromProject):
    def test_can_raise_warning_to_error(
        self, project, clear_project_flags, project_root, catcher: EventCatcher, runner: dbtRunner
    ) -> None:
        result = runner.invoke(["run"])
        self.assert_deprecation_warning(result, catcher)

        warn_error_options: Dict[str, Any] = {
            "flags": {"warn_error_options": {"error": ["DeprecatedModel"]}}
        }
        update_config_file(warn_error_options, project_root, "dbt_project.yml")

        catcher.flush()
        result = runner.invoke(["run"])
        self.assert_deprecation_error(result)

        warn_error_options = {
            "flags": {"warn_error_options": {"error": "all", "warn": ["DeprecationsSummary"]}}
        }
        update_config_file(warn_error_options, project_root, "dbt_project.yml")

        catcher.flush()
        result = runner.invoke(["run"])
        self.assert_deprecation_error(result)


class TestWarnErrorOptionsFromProjectCanExcludeSpecificEvent(BaseTestWarnErrorOptionsFromProject):
    @pytest.mark.skip(
        reason="Flaky on structured logging tests, EventCatcher inexplicably picks up on 'include' usage across classes"
    )
    def test_can_exclude_specific_event(
        self, project, clear_project_flags, project_root, catcher: EventCatcher, runner: dbtRunner
    ) -> None:
        warn_error_options: Dict[str, Any] = {
            "flags": {"warn_error_options": {"error": "all", "warn": ["DeprecationsSummary"]}}
        }
        update_config_file(warn_error_options, project_root, "dbt_project.yml")
        result = runner.invoke(["run"])
        self.assert_deprecation_error(result)

        warn_error_options = {
            "flags": {
                "warn_error_options": {
                    "error": "all",
                    "warn": ["DeprecatedModel", "DeprecationsSummary"],
                }
            }
        }
        update_config_file(warn_error_options, project_root, "dbt_project.yml")

        catcher.flush()
        result = runner.invoke(["run"])
        self.assert_deprecation_warning(result, catcher)


class TestWarnErrorOptionsFromProjectCantSetBothIncludeAndError(
    BaseTestWarnErrorOptionsFromProject
):
    def test_cant_set_both_include_and_error(
        self, project, clear_project_flags, project_root, runner: dbtRunner
    ) -> None:
        warn_error_options = {"flags": {"warn_error_options": {"include": "all", "error": "all"}}}
        update_config_file(warn_error_options, project_root, "dbt_project.yml")
        result = runner.invoke(["run"])
        assert not result.success
        assert result.exception is not None
        assert "Only `include` or `error` can be specified" in str(result.exception)


class TestWarnErrorOptionsFromProjectCantSetBothExcludeAndWarn(
    BaseTestWarnErrorOptionsFromProject
):
    def test_cant_set_both_exclude_and_warn(
        self, project, clear_project_flags, project_root, runner: dbtRunner
    ) -> None:
        warn_error_options = {
            "flags": {
                "warn_error_options": {
                    "error": "all",
                    "exclude": ["DeprecatedModel"],
                    "warn": ["DeprecatedModel"],
                }
            }
        }
        update_config_file(warn_error_options, project_root, "dbt_project.yml")
        result = runner.invoke(["run"])
        assert not result.success
        assert result.exception is not None
        assert "Only `exclude` or `warn` can be specified" in str(result.exception)


class TestEmptyWarnError:
    @pytest.fixture(scope="class")
    def models(self):
        return {"my_model.sql": my_model_sql, "schema.yml": schema_yml}

    # This tests for a bug in creating WarnErrorOptions when warn or
    # error are set to None (in yaml =  warn:)
    def test_project_flags(self, project):
        project_flags = {
            "flags": {
                "send_anonymous_usage_stats": False,
                "warn_error_options": {
                    "warn": None,
                    "error": None,
                    "silence": ["TestsConfigDeprecation"],
                },
            }
        }
        update_config_file(project_flags, project.project_root, "dbt_project.yml")
        run_dbt(["run"])
        flags = get_flags()
        assert flags.warn_error_options.silence == ["TestsConfigDeprecation"]


input_model_without_event_time_sql = """
{{ config(materialized='table') }}

select 1 as id, TIMESTAMP '2020-01-01 00:00:00-0' as event_time
union all
select 2 as id, TIMESTAMP '2020-01-02 00:00:00-0' as event_time
union all
select 3 as id, TIMESTAMP '2020-01-03 00:00:00-0' as event_time
"""

microbatch_model_sql = """
{{config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='day', begin=modules.datetime.datetime.now())}}
SELECT id, event_time FROM {{ ref('input_model') }}
"""


class TestRequireAllWarningsHandledByWarnErrorBehaviorFlag:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_without_event_time_sql,
            "microbatch_model.sql": microbatch_model_sql,
        }

    def test_require_all_warnings_handed_by_warn_error_behavior_flag(self, project):
        # Setup the event catchers
        microbatch_warning_catcher = EventCatcher(event_to_catch=MicrobatchModelNoEventTimeInputs)
        microbatch_error_catcher = EventCatcher(event_to_catch=MainEncounteredError)
        dbt_runner = dbtRunner(
            callbacks=[microbatch_warning_catcher.catch, microbatch_error_catcher.catch]
        )

        # Run the command without the behavior flag off
        project_flags = {
            "flags": {
                "send_anonymous_usage_stats": False,
                "require_all_warnings_handled_by_warn_error": False,
            }
        }
        update_config_file(project_flags, project.project_root, "dbt_project.yml")
        dbt_runner.invoke(["run", "--warn-error"])

        assert len(microbatch_warning_catcher.caught_events) == 1
        assert len(microbatch_error_catcher.caught_events) == 0

        # Reset the event catchers
        microbatch_warning_catcher.flush()
        microbatch_error_catcher.flush()

        # Run the command with the behavior flag on
        project_flags = {
            "flags": {
                "send_anonymous_usage_stats": False,
                "require_all_warnings_handled_by_warn_error": True,
            }
        }
        update_config_file(project_flags, project.project_root, "dbt_project.yml")
        dbt_runner.invoke(["run", "--warn-error", "--log-format", "json"])

        assert len(microbatch_warning_catcher.caught_events) == 0
        assert len(microbatch_error_catcher.caught_events) == 1


class TestWarnErrorOptionsToleratesV2FromCLI(BaseTestWarnErrorOptions):
    """dbt-core should ignore (not error on) v2-only warn_error_options names."""

    def test_v2_only_name_is_ignored_not_errored(
        self, project, catcher: EventCatcher, runner: dbtRunner
    ) -> None:
        # 'StaticAnalysis' is specific to the dbt v2 engine. dbt-core should
        # ignore it rather than raise, while still honoring the real 'DeprecatedModel'.
        result = runner.invoke(
            ["run", "--warn-error-options", "{'silence': ['StaticAnalysis', 'DeprecatedModel']}"]
        )
        assert result.success
        assert len(catcher.caught_events) == 0  # DeprecatedModel was silenced as requested

    def test_unknown_name_still_errors(self, project, runner: dbtRunner) -> None:
        result = runner.invoke(
            ["run", "--warn-error-options", "{'silence': ['TotallyBogusName']}"]
        )
        assert not result.success
        assert result.exception is not None
        assert "not a valid dbt error name" in str(result.exception)


class TestWarnErrorOptionsToleratesV2FromProject(BaseTestWarnErrorOptionsFromProject):
    def test_v2_only_name_is_ignored_not_errored(
        self, project, clear_project_flags, project_root, runner: dbtRunner
    ) -> None:
        # A v2-only name in dbt_project.yml should be ignored (with a Note,
        # see the convert_config unit test) rather than raising. Here we assert
        # the resulting behavior: the run succeeds and the name is stripped from
        # the resolved options. (The Note is emitted during Flags construction,
        # before the runner attaches its callbacks, so it can't be caught here.)
        options: Dict[str, Any] = {"flags": {"warn_error_options": {"error": ["StaticAnalysis"]}}}
        update_config_file(options, project_root, "dbt_project.yml")

        result = runner.invoke(["run"])
        assert result.success
        assert get_flags().warn_error_options.error == []

    def test_unknown_name_still_errors(
        self, project, clear_project_flags, project_root, runner: dbtRunner
    ) -> None:
        options: Dict[str, Any] = {
            "flags": {"warn_error_options": {"error": ["TotallyBogusName"]}}
        }
        update_config_file(options, project_root, "dbt_project.yml")

        result = runner.invoke(["run"])
        assert not result.success
        assert result.exception is not None
        assert "not a valid dbt error name" in str(result.exception)
