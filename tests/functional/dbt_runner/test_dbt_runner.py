import os
from unittest import mock

import pytest
from opentelemetry import trace
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter

from dbt.adapters.factory import FACTORY, reset_adapters
from dbt.cli.exceptions import DbtUsageException
from dbt.cli.main import dbtRunner
from dbt.exceptions import DbtProjectError
from dbt.tests.util import read_file, write_file
from dbt.version import __version__ as dbt_version
from dbt_common.events.contextvars import get_node_info


class TestDbtRunner:
    @pytest.fixture
    def dbt(self) -> dbtRunner:
        return dbtRunner()

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "models.sql": "select 1 as id",
        }

    def test_group_invalid_option(self, dbt: dbtRunner) -> None:
        res = dbt.invoke(["--invalid-option"])
        assert type(res.exception) == DbtUsageException

    def test_command_invalid_option(self, dbt: dbtRunner) -> None:
        res = dbt.invoke(["deps", "--invalid-option"])
        assert type(res.exception) == DbtUsageException

    def test_command_mutually_exclusive_option(self, dbt: dbtRunner) -> None:
        res = dbt.invoke(["--warn-error", "--warn-error-options", '{"include": "all"}', "deps"])
        assert type(res.exception) == DbtUsageException
        res = dbt.invoke(["deps", "--warn-error", "--warn-error-options", '{"include": "all"}'])
        assert type(res.exception) == DbtUsageException

        res = dbt.invoke(["compile", "--select", "models", "--inline", "select 1 as id"])
        assert type(res.exception) == DbtUsageException

    def test_invalid_command(self, dbt: dbtRunner) -> None:
        res = dbt.invoke(["invalid-command"])
        assert type(res.exception) == DbtUsageException

    def test_invoke_version(self, dbt: dbtRunner) -> None:
        dbt.invoke(["--version"])

    def test_callbacks(self) -> None:
        mock_callback = mock.MagicMock()
        dbt = dbtRunner(callbacks=[mock_callback])
        # the `debug` command is one of the few commands wherein you don't need
        # to have a project to run it and it will emit events
        dbt.invoke(["debug"])
        mock_callback.assert_called()

    def test_invoke_kwargs(self, project, dbt):
        res = dbt.invoke(
            ["run"],
            log_format="json",
            log_path="some_random_path",
            version_check=False,
            profile_name="some_random_profile_name",
            target_dir="some_random_target_dir",
        )
        assert res.result.args["log_format"] == "json"
        assert res.result.args["log_path"] == "some_random_path"
        assert res.result.args["version_check"] is False
        assert res.result.args["profile_name"] == "some_random_profile_name"
        assert res.result.args["target_dir"] == "some_random_target_dir"

    def test_invoke_kwargs_project_dir(self, project, dbt):
        res = dbt.invoke(["run"], project_dir="some_random_project_dir")
        assert type(res.exception) == DbtProjectError

        msg = "No dbt_project.yml found at expected path some_random_project_dir"
        assert msg in res.exception.msg

    def test_invoke_kwargs_profiles_dir(self, project, dbt):
        res = dbt.invoke(["run"], profiles_dir="some_random_profiles_dir")
        assert type(res.exception) == DbtProjectError
        msg = "Could not find profile named 'test'"
        assert msg in res.exception.msg

    def test_invoke_kwargs_and_flags(self, project, dbt):
        res = dbt.invoke(["--log-format=text", "run"], log_format="json")
        assert res.result.args["log_format"] == "json"

    def test_pass_in_manifest(self, project, dbt):
        result = dbt.invoke(["parse"])
        manifest = result.result

        reset_adapters()
        assert len(FACTORY.adapters) == 0
        result = dbtRunner(manifest=manifest).invoke(["run"])
        # Check that the adapters are registered again.
        assert result.success
        assert len(FACTORY.adapters) == 1

    def test_pass_in_args_variable(self, dbt):
        args = ["--log-format", "text"]
        args_before = args.copy()
        dbt.invoke(args)
        assert args == args_before

    def test_directory_does_not_change(self, project, dbt: dbtRunner) -> None:
        project_dir = os.getcwd()  # The directory where dbt_project.yml exists.
        os.chdir("../")
        cmd_execution_dir = os.getcwd()  # The directory where dbt command will be run

        commands = ["init", "deps", "clean"]
        for command in commands:
            args = [command, "--project-dir", project_dir]
            if command == "init":
                args.append("--skip-profile-setup")
            res = dbt.invoke(args)
            after_dir = os.getcwd()
            assert res.success is True
            assert cmd_execution_dir == after_dir


class TestDbtRunnerQueryComments:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "models.sql": "select 1 as id",
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "query-comment": {
                "comment": f"comment: {dbt_version}",
                "append": True,
            }
        }

    def test_query_comment_saved_manifest(self, project, logs_dir):
        dbt = dbtRunner()
        dbt.invoke(["build", "--select", "models"])
        result = dbt.invoke(["parse"])
        write_file("", logs_dir, "dbt.log")
        # pass in manifest from parse command
        dbt = dbtRunner(result.result)
        dbt.invoke(["build", "--select", "models"])
        log_file = read_file(logs_dir, "dbt.log")
        assert f"comment: {dbt_version}" in log_file


class TestDbtRunnerHooks:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "models.sql": "select 1 as id",
            "model2.sql": "select * from {{ ref('models') }}",
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"on-run-end": ["select 1;"]}

    def test_node_info_non_persistence(self, project):
        dbt = dbtRunner()
        dbt.invoke(["run", "--select", "models"])
        assert get_node_info() == {}

    def test_dbt_runner_spans(self, project):
        tracer_provider = TracerProvider(resource=Resource.get_empty())
        span_exporter = InMemorySpanExporter()
        trace.set_tracer_provider(tracer_provider)
        trace.get_tracer_provider().add_span_processor(SimpleSpanProcessor(span_exporter))
        dbt = dbtRunner()
        dbt.invoke(["run", "--select", "models", "model2"])
        assert get_node_info() == {}
        exported_spans = span_exporter.get_finished_spans()
        assert len(exported_spans) == 3
        assert exported_spans[0].instrumentation_scope.name == "com.dbt.runner"
        span_names = [span.name for span in exported_spans]
        span_names.sort()
        assert span_names == ["model.test.model2", "model.test.models", "on-run-end"]
        model2_span = None
        models_span = None
        for span in exported_spans:
            if span.name == "model.test.model2":
                model2_span = span
            if span.name == "model.test.models":
                models_span = span

        # verify span links
        assert len(model2_span.links) == 1
        assert model2_span.links[0].attributes["parent_model_fqn"] == "model.test.models"
        assert model2_span.links[0].context.span_id == models_span.context.span_id
        assert model2_span.links[0].context.trace_id == models_span.context.trace_id
