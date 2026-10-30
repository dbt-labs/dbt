import os

import pytest
from pytest_mock import MockerFixture

from dbt.events.types import JinjaLogInfo, PartialParsingNotEnabled
from dbt.tests.util import run_dbt
from dbt_common.events.event_catcher import EventCatcher

model_one_sql = """
    {{ log("DBT_ENGINE_SHOW_RESOURCE_REPORT: " ~ env_var('DBT_ENGINE_SHOW_RESOURCE_REPORT', default="0"), info=True) }}
    {{ log("DBT_SHOW_RESOURCE_REPORT: " ~ env_var('DBT_SHOW_RESOURCE_REPORT', default="0"), info=True) }}
    select 1 as fun
"""


class TestOldEngineEnvVarPropagation:
    @pytest.fixture(scope="class")
    def models(self):
        return {"model_one.sql": model_one_sql}

    @pytest.mark.parametrize(
        "set_old,set_new, expect",
        [(False, False, 0), (True, False, False), (False, True, True), (True, True, True)],
    )
    def test_engine_env_var_propagation(
        self, project, mocker: MockerFixture, set_old: bool, set_new: bool, expect: bool
    ):
        # Of note, the default value for DBT_PARTIAL_PARSE is True
        if set_old:
            mocker.patch.dict(os.environ, {"DBT_SHOW_RESOURCE_REPORT": "False"})
        if set_new:
            mocker.patch.dict(os.environ, {"DBT_ENGINE_SHOW_RESOURCE_REPORT": "True"})

        event_catcher = EventCatcher(event_to_catch=JinjaLogInfo)
        run_dbt(["parse", "--no-partial-parse"], callbacks=[event_catcher.catch])

        assert len(event_catcher.caught_events) == 2

        for event in event_catcher.caught_events:
            if event.data.msg.startswith("DBT_ENGINE_SHOW_RESOURCE_REPORT"):
                assert event.data.msg.endswith(f"{expect}")
            elif event.data.msg.startswith("DBT_SHOW_RESOURCE_REPORT"):
                assert event.data.msg.endswith(f"{expect}")
            else:
                assert False, "Unexpected log message"


class TestEngineEnvVarPickedUpByClick:
    @pytest.fixture(scope="class")
    def models(self):
        return {"model_one.sql": model_one_sql}

    def test_engine_env_var_picked_up_by_cli_flags(self, project, mocker: MockerFixture):
        event_catcher = EventCatcher(event_to_catch=PartialParsingNotEnabled)

        run_dbt(["parse"], callbacks=[event_catcher.catch])
        assert len(event_catcher.caught_events) == 0

        run_dbt(["parse"], callbacks=[event_catcher.catch])
        assert len(event_catcher.caught_events) == 0

        mocker.patch.dict(os.environ, {"DBT_ENGINE_PARTIAL_PARSE": "False"})
        run_dbt(["parse"], callbacks=[event_catcher.catch])
        assert len(event_catcher.caught_events) == 1


class TestKnownEngineEnvVarsExplicit:
    def test_allow_list_is_correct(self, project):
        run_dbt(["parse"])

        hard_coded_allow_list = {
            "DBT_ENGINE_NO_PRINT",
            "DBT_ENGINE_EXCLUDE_RESOURCE_TYPES",
            "DBT_ENGINE_TARGET",
            "DBT_ENGINE_PROJECT_DIR",
            "DBT_ENGINE_MACRO_DEBUGGING",
            "DBT_ENGINE_EVENT_TIME_END",
            "DBT_ENGINE_PRINTER_WIDTH",
            "DBT_ENGINE_PACKAGE_HUB_URL",
            "DBT_ENGINE_TARGET_PATH",
            "DBT_ENGINE_EMPTY",
            "DBT_ENGINE_DOWNLOAD_DIR",
            "DBT_ENGINE_INDIRECT_SELECTION",
            "DBT_ENGINE_SHOW_RESOURCE_REPORT",
            "DBT_ENGINE_EVENT_TIME_START",
            "DBT_ENGINE_LOG_CACHE_EVENTS",
            "DBT_ENGINE_USE_COLORS_FILE",
            "DBT_ENGINE_LOG_FILE_MAX_BYTES",
            "DBT_ENGINE_DEFER_TO_STATE",
            "DBT_ENGINE_RESOURCE_TYPES",
            "DBT_ENGINE_HOST",
            "DBT_ENGINE_STATE",
            "DBT_ENGINE_PP_FILE_DIFF_TEST",
            "DBT_ENGINE_STORE_FAILURES",
            "DBT_ENGINE_LOG_PATH",
            "DBT_ENGINE_EXPORT_SAVED_QUERIES",
            "DBT_ENGINE_CLEAN_PROJECT_FILES_ONLY",
            "DBT_ENGINE_CACHE_SELECTED_ONLY",
            "DBT_ENGINE_WRITE_JSON",
            "DBT_ENGINE_SEND_ANONYMOUS_USAGE_STATS",
            "DBT_ENGINE_RECORDED_FILE_PATH",
            "DBT_ENGINE_PARTIAL_PARSE_FILE_DIFF",
            "DBT_ENGINE_PP_TEST",
            "DBT_ENGINE_INTROSPECT",
            "DBT_ENGINE_USE_FAST_TEST_EDGES",
            "DBT_ENGINE_VERSION_CHECK",
            "DBT_ENGINE_QUIET",
            "DBT_ENGINE_SINGLE_THREADED",
            "DBT_ENGINE_SNOWFLAKE_PROJECTS_OTEL",
            "DBT_ENGINE_SQLPARSE",
            "DBT_ENGINE_ARTIFACT_STATE_PATH",
            "DBT_ENGINE_FULL_REFRESH",
            "DBT_ENGINE_FAIL_FAST",
            "DBT_ENGINE_INCLUDE_SAVED_QUERY",
            "DBT_ENGINE_WARN_ERROR",
            "DBT_ENGINE_PROFILES_DIR",
            "DBT_ENGINE_LOG_LEVEL",
            "DBT_ENGINE_STATIC_PARSER",
            "DBT_ENGINE_PROFILE",
            "DBT_ENGINE_PARTIAL_PARSE",
            "DBT_ENGINE_POPULATE_CACHE",
            "DBT_ENGINE_DEFER",
            "DBT_ENGINE_FAVOR_STATE_MODE",
            "DBT_ENGINE_USE_COLORS",
            "DBT_ENGINE_DEFER_STATE",
            "DBT_ENGINE_LOG_FORMAT",
            "DBT_ENGINE_PARTIAL_PARSE_FILE_PATH",
            "DBT_ENGINE_WARN_ERROR_OPTIONS",
            "DBT_ENGINE_INVOCATION_ENV",
            "DBT_ENGINE_FAVOR_STATE",
            "DBT_ENGINE_LOG_FORMAT_FILE",
            "DBT_ENGINE_TEST_STATE_MODIFIED",
            "DBT_ENGINE_LOG_LEVEL_FILE",
            "DBT_ENGINE_USE_EXPERIMENTAL_PARSER",
            "DBT_ENGINE_USE_V2_PARSER",
            "DBT_ENGINE_V2_PARSER",
            "DBT_ENGINE_UPLOAD_TO_ARTIFACTS_INGEST_API",
            "DBT_ENGINE_DEBUG",
            "DBT_ENGINE_PRINT",
            "DBT_ENGINE_SAMPLE",
            "DBT_ENGINE_MANAGE_STATE",
            "DBT_ENGINE_STATE_OAUTH_CLIENT_ID",
            "DBT_ENGINE_STATE_AUTH_URL",
            "DBT_ENGINE_STATE_TOKEN_URL",
            "DBT_ENGINE_STATE_API_URL",
            "DBT_ENGINE_MANAGE_STATE",
            "DBT_ENGINE_SKIP_BROWSER_AUTH",
            "DBT_ENGINE_HINTS_ENABLED",
        }
        from dbt.env_vars import _ALLOWED_ENV_VARS

        assert hard_coded_allow_list == _ALLOWED_ENV_VARS
