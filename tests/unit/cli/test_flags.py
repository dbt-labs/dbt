from pathlib import Path
from typing import List, Optional
from unittest import mock

import click
import pytest

from dbt.cli.exceptions import DbtUsageException
from dbt.cli.flags import Flags
from dbt.cli.main import cli
from dbt.cli.types import Command
from dbt.contracts.project import ProjectFlags
from dbt.tests.util import rm_file, write_file
from dbt_common.exceptions import DbtInternalError
from dbt_common.helper_types import WarnErrorOptionsV2


class TestFlags:
    def make_dbt_context(
        self, context_name: str, args: List[str], parent: Optional[click.Context] = None
    ) -> click.Context:
        ctx = cli.make_context(context_name, args.copy(), parent)
        return ctx

    @pytest.fixture(scope="class")
    def run_context(self) -> click.Context:
        return self.make_dbt_context("run", ["run"])

    @pytest.fixture
    def project_flags(self) -> ProjectFlags:
        return ProjectFlags()

    def test_cli_args_unmodified(self):
        args = ["--target", "my_target"]
        args_before = args.copy()
        self.make_dbt_context("context", args)

        assert args == args_before

    def test_which(self, run_context):
        flags = Flags(run_context)
        assert flags.WHICH == "run"

    @pytest.mark.parametrize("param", cli.params)
    def test_cli_group_flags_from_params(self, run_context, param):
        with mock.patch("dbt.cli.flags.get_user_setting_flags", return_value={}):
            flags = Flags(run_context)

        if "DEPRECATED_" in param.name.upper():
            assert not hasattr(flags, param.name.upper())
            return

        if param.name.upper() in ("VERSION", "LOG_PATH"):
            return

        assert hasattr(flags, param.name.upper())
        assert getattr(flags, param.name.upper()) == run_context.params[param.name.lower()]

    def test_log_path_default(self, run_context):
        flags = Flags(run_context)
        assert hasattr(flags, "LOG_PATH")
        assert getattr(flags, "LOG_PATH") == Path("logs")

    def test_log_file_max_size_default(self, run_context):
        flags = Flags(run_context)
        assert hasattr(flags, "LOG_FILE_MAX_BYTES")
        assert getattr(flags, "LOG_FILE_MAX_BYTES") == 10 * 1024 * 1024

    @pytest.mark.parametrize(
        "set_stats_param,do_not_track,expected_anonymous_usage_stats",
        [
            # set_stats_param = default, DNT = True, expected = False
            ("default", "1", False),
            ("default", "t", False),
            ("default", "true", False),
            ("default", "y", False),
            ("default", "yes", False),
            # set_stats_param = default, DNT = false, expected = True
            ("default", "false", True),
            ("default", "anything", True),
            # set_stats_param = True, DNT = True, expected = False
            (True, "1", False),
            (True, "t", False),
            (True, "true", False),
            (True, "y", False),
            (True, "yes", False),
            # set_stats_param = True, DNT = false, expected = True
            (True, "false", True),
            (True, "anything", True),
            (True, "2", True),
            # set_stats_param = False, DNT = True, expected = False
            (False, "1", False),
            (False, "t", False),
            (False, "true", False),
            (False, "y", False),
            (False, "yes", False),
            # set_stats_param = False, DNT = False, expected = False
            (False, "false", False),
            (False, "anything", False),
            (False, "2", False),
        ],
    )
    def test_anonymous_usage_state(
        self,
        monkeypatch,
        run_context,
        set_stats_param,
        do_not_track,
        expected_anonymous_usage_stats,
    ):
        monkeypatch.setenv("DO_NOT_TRACK", do_not_track)
        if set_stats_param != "default":
            run_context.params["send_anonymous_usage_stats"] = set_stats_param
        flags = Flags(run_context)
        assert flags.SEND_ANONYMOUS_USAGE_STATS == expected_anonymous_usage_stats

    def test_resource_types(self, monkeypatch):
        monkeypatch.setenv("DBT_RESOURCE_TYPES", "model")
        build_context = self.make_dbt_context("build", ["build"])
        build_context.params["resource_types"] = ("unit_test",)
        flags = Flags(build_context)
        assert flags.resource_types == ("unit_test",)

    def test_empty_project_flags_uses_default(self, run_context, project_flags):
        flags = Flags(run_context, project_flags)
        assert flags.USE_COLORS == run_context.params["use_colors"]

    def test_none_project_flags_uses_default(self, run_context):
        flags = Flags(run_context, None)
        assert flags.USE_COLORS == run_context.params["use_colors"]

    def test_prefer_project_flags_to_default(self, run_context, project_flags):
        project_flags.use_colors = False
        # ensure default value is not the same as user config
        assert run_context.params["use_colors"] is not project_flags.use_colors

        flags = Flags(run_context, project_flags)
        assert flags.USE_COLORS == project_flags.use_colors

    def test_prefer_param_value_to_project_flags(self):
        project_flags = ProjectFlags(use_colors=False)
        context = self.make_dbt_context("run", ["--use-colors", "True", "run"])

        flags = Flags(context, project_flags)
        assert flags.USE_COLORS

    def test_prefer_env_to_project_flags(self, monkeypatch, project_flags):
        project_flags.use_colors = False
        monkeypatch.setenv("DBT_USE_COLORS", "True")
        context = self.make_dbt_context("run", ["run"])

        flags = Flags(context, project_flags)
        assert flags.USE_COLORS

    def test_prefer_user_settings_to_default(self, monkeypatch):
        monkeypatch.setattr(
            "dbt.cli.flags.get_user_setting_flags",
            lambda: {"use_colors": False},
        )
        context = self.make_dbt_context("run", ["run"])
        flags = Flags(context)
        assert flags.USE_COLORS is False

    def test_prefer_project_flags_to_user_settings(self, monkeypatch, project_flags):
        project_flags.use_colors = True
        monkeypatch.setattr(
            "dbt.cli.flags.get_user_setting_flags",
            lambda: {"use_colors": False},
        )
        context = self.make_dbt_context("run", ["run"])
        flags = Flags(context, project_flags)
        assert flags.USE_COLORS is True

    def test_prefer_cli_to_user_settings(self, monkeypatch):
        monkeypatch.setattr(
            "dbt.cli.flags.get_user_setting_flags",
            lambda: {"use_colors": False},
        )
        context = self.make_dbt_context("run", ["--use-colors", "True", "run"])
        flags = Flags(context)
        assert flags.USE_COLORS is True

    @pytest.mark.parametrize(
        "argv,manage_state_project,user_flags,env_value,expected_state,expected_source",
        [
            (["run"], None, {}, None, False, None),
            (["--manage-state", "run"], None, {}, None, True, "cli_flag"),
            (["run"], None, {}, "True", True, "env_var"),
            (["run"], True, {}, None, True, "project_config"),
            (["run"], None, {"manage_state": True}, None, True, "user_settings"),
            (["--manage-state", "run"], False, {}, None, True, "cli_flag"),
        ],
        ids=[
            "unset",
            "cli_flag",
            "env_var",
            "project_config",
            "user_settings",
            "cli_overrides_project",
        ],
    )
    def test_manage_state_source(
        self,
        monkeypatch,
        argv,
        manage_state_project,
        user_flags,
        env_value,
        expected_state,
        expected_source,
    ):
        # PluginManager may leak DBT_ENGINE_MANAGE_STATE=true into os.environ in a
        # shared test process; control it explicitly per case.
        if env_value is None:
            monkeypatch.delenv("DBT_ENGINE_MANAGE_STATE", raising=False)
        else:
            monkeypatch.setenv("DBT_ENGINE_MANAGE_STATE", env_value)
        monkeypatch.setattr("dbt.cli.flags.get_user_setting_flags", lambda: user_flags)
        context = self.make_dbt_context("run", argv)
        if manage_state_project is None:
            flags = Flags(context)
        else:
            flags = Flags(context, ProjectFlags(manage_state=manage_state_project))
        assert flags.MANAGE_STATE is expected_state
        assert flags.MANAGE_STATE_SOURCE == expected_source

    def test_manage_state_source_programmatic(self, monkeypatch):
        # dbtRunner.invoke(manage_state=True) tags the param source with the string
        # "kwargs"; this should be attributed to the "programmatic" surface.
        monkeypatch.delenv("DBT_ENGINE_MANAGE_STATE", raising=False)
        monkeypatch.setattr("dbt.cli.flags.get_user_setting_flags", lambda: {})
        context = self.make_dbt_context("run", ["run"])
        context.params["manage_state"] = True
        context.set_parameter_source("manage_state", "kwargs")
        flags = Flags(context)
        assert flags.MANAGE_STATE is True
        assert flags.MANAGE_STATE_SOURCE == "programmatic"

    def test_mutually_exclusive_options_passed_separately(self):
        """Assert options that are mutually exclusive can be passed separately without error"""
        warn_error_context = self.make_dbt_context("run", ["--warn-error", "run"])

        flags = Flags(warn_error_context)
        assert flags.WARN_ERROR

        warn_error_options_context = self.make_dbt_context(
            "run", ["--warn-error-options", '{"error": "all"}', "run"]
        )
        flags = Flags(warn_error_options_context)
        assert flags.WARN_ERROR_OPTIONS == WarnErrorOptionsV2(error="all")

    def test_mutually_exclusive_options_from_cli(self):
        context = self.make_dbt_context(
            "run", ["--warn-error", "--warn-error-options", '{"error": "all"}', "run"]
        )

        with pytest.raises(DbtUsageException):
            Flags(context)

    @pytest.mark.parametrize("warn_error", [True, False])
    def test_mutually_exclusive_options_from_project_flags(self, warn_error, project_flags):
        project_flags.warn_error = warn_error
        context = self.make_dbt_context("run", ["--warn-error-options", '{"error": "all"}', "run"])

        with pytest.raises(DbtUsageException):
            Flags(context, project_flags)

    @pytest.mark.parametrize("warn_error", ["True", "False"])
    def test_mutually_exclusive_options_from_envvar(self, warn_error, monkeypatch):
        monkeypatch.setenv("DBT_WARN_ERROR", warn_error)
        monkeypatch.setenv("DBT_WARN_ERROR_OPTIONS", '{"error":"all"}')
        context = self.make_dbt_context("run", ["run"])

        with pytest.raises(DbtUsageException):
            Flags(context)

    @pytest.mark.parametrize("warn_error", [True, False])
    def test_mutually_exclusive_options_from_cli_and_project_flags(
        self, warn_error, project_flags
    ):
        project_flags.warn_error = warn_error
        context = self.make_dbt_context("run", ["--warn-error-options", '{"error": "all"}', "run"])

        with pytest.raises(DbtUsageException):
            Flags(context, project_flags)

    @pytest.mark.parametrize("warn_error", ["True", "False"])
    def test_mutually_exclusive_options_from_cli_and_envvar(self, warn_error, monkeypatch):
        monkeypatch.setenv("DBT_WARN_ERROR", warn_error)
        context = self.make_dbt_context("run", ["--warn-error-options", '{"error": "all"}', "run"])

        with pytest.raises(DbtUsageException):
            Flags(context)

    @pytest.mark.parametrize("warn_error", ["True", "False"])
    def test_mutually_exclusive_options_from_project_flags_and_envvar(
        self, project_flags, warn_error, monkeypatch
    ):
        project_flags.warn_error = warn_error
        monkeypatch.setenv("DBT_WARN_ERROR_OPTIONS", '{"error": "all"}')
        context = self.make_dbt_context("run", ["run"])

        with pytest.raises(DbtUsageException):
            Flags(context, project_flags)

    @pytest.mark.parametrize(
        "cli_colors,cli_colors_file,flag_colors,flag_colors_file",
        [
            (None, None, True, True),
            (True, None, True, True),
            (None, True, True, True),
            (False, None, False, False),
            (None, False, True, False),
            (True, True, True, True),
            (False, False, False, False),
            (True, False, True, False),
            (False, True, False, True),
        ],
    )
    def test_no_color_interaction(
        self, cli_colors, cli_colors_file, flag_colors, flag_colors_file
    ):
        cli_params = []

        if cli_colors is not None:
            cli_params.append("--use-colors" if cli_colors else "--no-use-colors")

        if cli_colors_file is not None:
            cli_params.append("--use-colors-file" if cli_colors_file else "--no-use-colors-file")

        cli_params.append("run")

        context = self.make_dbt_context("run", cli_params)

        flags = Flags(context, None)

        assert flags.USE_COLORS == flag_colors
        assert flags.USE_COLORS_FILE == flag_colors_file

    @pytest.mark.parametrize(
        "cli_log_level,cli_log_level_file,flag_log_level,flag_log_level_file",
        [
            (None, None, "info", "debug"),
            ("error", None, "error", "error"),  # explicit level overrides file level...
            ("info", None, "info", "info"),  # ...but file level doesn't change console level
            (
                "debug",
                "warn",
                "debug",
                "warn",
            ),  # still, two separate explicit levels are applied independently
        ],
    )
    def test_log_level_interaction(
        self, cli_log_level, cli_log_level_file, flag_log_level, flag_log_level_file
    ):
        cli_params = []

        if cli_log_level is not None:
            cli_params.append("--log-level")
            cli_params.append(cli_log_level)

        if cli_log_level_file is not None:
            cli_params.append("--log-level-file")
            cli_params.append(cli_log_level_file)

        cli_params.append("run")

        context = self.make_dbt_context("run", cli_params)

        flags = Flags(context, None)

        assert flags.LOG_LEVEL == flag_log_level
        assert flags.LOG_LEVEL_FILE == flag_log_level_file

    @pytest.mark.parametrize(
        "cli_log_format,cli_log_format_file,flag_log_format,flag_log_format_file",
        [
            (None, None, "default", "debug"),
            ("json", None, "json", "json"),  # explicit format overrides file format...
            (None, "json", "default", "json"),  # ...but file format doesn't change console format
            (
                "debug",
                "text",
                "debug",
                "text",
            ),  # still, two separate explicit formats are applied independently
        ],
    )
    def test_log_format_interaction(
        self, cli_log_format, cli_log_format_file, flag_log_format, flag_log_format_file
    ):
        cli_params = []

        if cli_log_format is not None:
            cli_params.append("--log-format")
            cli_params.append(cli_log_format)

        if cli_log_format_file is not None:
            cli_params.append("--log-format-file")
            cli_params.append(cli_log_format_file)

        cli_params.append("run")

        context = self.make_dbt_context("run", cli_params)

        flags = Flags(context, None)

        assert flags.LOG_FORMAT == flag_log_format
        assert flags.LOG_FORMAT_FILE == flag_log_format_file

    def test_log_settings_from_config(self):
        """Test that values set in ProjectFlags for log settings will set flags as expected"""
        context = self.make_dbt_context("run", ["run"])

        config = ProjectFlags(log_format="json", log_level="warn", use_colors=False)

        flags = Flags(context, config)

        assert flags.LOG_FORMAT == "json"
        assert flags.LOG_FORMAT_FILE == "json"
        assert flags.LOG_LEVEL == "warn"
        assert flags.LOG_LEVEL_FILE == "warn"
        assert flags.USE_COLORS is False
        assert flags.USE_COLORS_FILE is False

    def test_log_file_settings_from_config(self):
        """Test that values set in ProjectFlags for log *file* settings will set flags as expected, leaving the console
        logging flags with their default values"""
        context = self.make_dbt_context("run", ["run"])

        config = ProjectFlags(log_format_file="json", log_level_file="warn", use_colors_file=False)

        flags = Flags(context, config)

        assert flags.LOG_FORMAT == "default"
        assert flags.LOG_FORMAT_FILE == "json"
        assert flags.LOG_LEVEL == "info"
        assert flags.LOG_LEVEL_FILE == "warn"
        assert flags.USE_COLORS is True
        assert flags.USE_COLORS_FILE is False

    def test_duplicate_flags_raises_error(self):
        parent_context = self.make_dbt_context("parent", ["--version-check"])
        context = self.make_dbt_context("child", ["--version-check"], parent_context)

        with pytest.raises(DbtUsageException):
            Flags(context)

    def test_global_flag_at_child_context(self):
        parent_context_a = self.make_dbt_context("parent_context_a", ["--no-use-colors"])
        child_context_a = self.make_dbt_context("child_context_a", ["run"], parent_context_a)
        flags_a = Flags(child_context_a)

        parent_context_b = self.make_dbt_context("parent_context_b", ["run"])
        child_context_b = self.make_dbt_context(
            "child_context_b", ["--no-use-colors"], parent_context_b
        )
        flags_b = Flags(child_context_b)

        assert flags_a.USE_COLORS == flags_b.USE_COLORS

    def test_global_flag_with_env_var(self, monkeypatch):
        # The environment variable is used for whichever parent or child
        # does not have a cli command.
        # Test that "child" global flag overrides env var
        monkeypatch.setenv("DBT_QUIET", "0")
        parent_context = self.make_dbt_context("parent", ["--no-use-colors"])
        child_context = self.make_dbt_context("child", ["--quiet"], parent_context)
        flags = Flags(child_context)
        assert flags.QUIET is True

        # Test that "parent" global flag overrides env var
        parent_context = self.make_dbt_context("parent", ["--quiet"])
        child_context = self.make_dbt_context("child", ["--no-use-colors"], parent_context)
        flags = Flags(child_context)
        assert flags.QUIET is True

    def test_set_project_only_flags(self, project_flags, run_context):
        flags = Flags(run_context, project_flags)

        for project_only_flag, project_only_flag_value in project_flags.project_only_flags.items():
            assert getattr(flags, project_only_flag) == project_only_flag_value
            # sanity check: ensure project_only_flag is not part of the click context
            assert project_only_flag not in run_context.params

    def _create_flags_from_dict(self, cmd, d):
        write_file("", "profiles.yml")
        result = Flags.from_dict(cmd, d)
        assert result.which is cmd.value
        rm_file("profiles.yml")
        return result

    def test_from_dict__run(self):
        args_dict = {
            "print": False,
            "select": ["model_one", "model_two"],
        }
        result = self._create_flags_from_dict(Command.RUN, args_dict)
        assert "model_one" in result.select[0]
        assert "model_two" in result.select[1]

    def test_from_dict__build(self):
        args_dict = {
            "print": True,
            "state": "some/path",
            "defer_state": None,
        }
        result = self._create_flags_from_dict(Command.BUILD, args_dict)
        assert result.print is True
        assert "some/path" in str(result.state)
        assert result.defer_state is None

    def test_from_dict__seed(self):
        args_dict = {"use_colors": False, "exclude": ["model_three"]}
        result = self._create_flags_from_dict(Command.SEED, args_dict)
        assert result.use_colors is False
        assert "model_three" in result.exclude[0]

    def test_from_dict__which_fails(self):
        args_dict = {"which": "some bad command"}
        with pytest.raises(DbtInternalError, match=r"does not match value of which"):
            self._create_flags_from_dict(Command.RUN, args_dict)

    def test_from_dict_0_value(self):
        args_dict = {"log_file_max_bytes": 0}
        flags = Flags.from_dict(Command.RUN, args_dict)
        assert flags.LOG_FILE_MAX_BYTES == 0


def test_project_flag_defaults():
    flags = ProjectFlags()
    # From # 9183: Let's add a unit test that ensures that:
    # every attribute of ProjectFlags that has a corresponding click option
    # in params.py should be set to None by default (except for anon user
    # tracking). Going forward, flags can have non-None defaults if they
    # do not have a corresponding CLI option/env var. These will be used
    # to control backwards incompatible interface or behaviour changes.

    # List of all flags except send_anonymous_usage_stats
    project_flags = [
        "cache_selected_only",
        "debug",
        "fail_fast",
        "indirect_selection",
        "log_format",
        "log_format_file",
        "log_level",
        "log_level_file",
        "partial_parse",
        "populate_cache",
        "printer_width",
        "static_parser",
        "use_colors",
        "use_colors_file",
        "use_experimental_parser",
        "use_v2_parser",
        "v2_parser",
        "version_check",
        "warn_error",
        "warn_error_options",
        "write_json",
    ]
    for flag in project_flags:
        assert getattr(flags, flag) is None


class TestV2ParserFlags:
    def make_dbt_context(
        self, context_name: str, args: List[str], parent: Optional[click.Context] = None
    ) -> click.Context:
        return cli.make_context(context_name, args.copy(), parent)

    def test_default_off(self):
        ctx = self.make_dbt_context("run", ["run"])
        flags = Flags(ctx)
        assert flags.USE_V2_PARSER is False
        assert flags.V2_PARSER == "dbt-core-experimental-parser parse"

    def test_cli_arg_enables(self):
        ctx = self.make_dbt_context("run", ["--use-v2-parser", "run"])
        flags = Flags(ctx)
        assert flags.USE_V2_PARSER is True

    def test_cli_arg_command(self):
        ctx = self.make_dbt_context(
            "run",
            ["--v2-parser", "/opt/parser/bin/dbt-core-experimental-parser parse", "run"],
        )
        flags = Flags(ctx)
        assert flags.V2_PARSER == "/opt/parser/bin/dbt-core-experimental-parser parse"

    def test_env_var_enables(self, monkeypatch):
        monkeypatch.setenv("DBT_ENGINE_USE_V2_PARSER", "True")
        ctx = self.make_dbt_context("run", ["run"])
        flags = Flags(ctx)
        assert flags.USE_V2_PARSER is True

    def test_env_var_command(self, monkeypatch):
        monkeypatch.setenv("DBT_ENGINE_V2_PARSER", "dbt-core-experimental-parser parse --foo")
        ctx = self.make_dbt_context("run", ["run"])
        flags = Flags(ctx)
        assert flags.V2_PARSER == "dbt-core-experimental-parser parse --foo"

    def test_project_flags_set_use_v2_parser(self):
        project_flags = ProjectFlags(use_v2_parser=True)
        ctx = self.make_dbt_context("run", ["run"])
        flags = Flags(ctx, project_flags)
        assert flags.USE_V2_PARSER is True

    def test_project_flags_set_command(self):
        project_flags = ProjectFlags(v2_parser="dbt-core-experimental-parser parse --strict")
        ctx = self.make_dbt_context("run", ["run"])
        flags = Flags(ctx, project_flags)
        assert flags.V2_PARSER == "dbt-core-experimental-parser parse --strict"

    def test_cli_overrides_project_flags(self):
        project_flags = ProjectFlags(use_v2_parser=True)
        ctx = self.make_dbt_context("run", ["--no-use-v2-parser", "run"])
        flags = Flags(ctx, project_flags)
        assert flags.USE_V2_PARSER is False


class TestSnowflakeProjectsOtelFlag:
    def make_dbt_context(
        self, context_name: str, args: List[str], parent: Optional[click.Context] = None
    ) -> click.Context:
        return cli.make_context(context_name, args.copy(), parent)

    def test_default_off(self):
        ctx = self.make_dbt_context("run", ["run"])
        flags = Flags(ctx)
        assert flags.SNOWFLAKE_PROJECTS_OTEL is False

    def test_cli_arg_enables(self):
        ctx = self.make_dbt_context("run", ["--snowflake-projects-otel", "run"])
        flags = Flags(ctx)
        assert flags.SNOWFLAKE_PROJECTS_OTEL is True

    def test_env_var_enables(self, monkeypatch):
        monkeypatch.setenv("DBT_ENGINE_SNOWFLAKE_PROJECTS_OTEL", "True")
        ctx = self.make_dbt_context("run", ["run"])
        flags = Flags(ctx)
        assert flags.SNOWFLAKE_PROJECTS_OTEL is True

    def test_project_flags_enables(self):
        project_flags = ProjectFlags(snowflake_projects_otel=True)
        ctx = self.make_dbt_context("run", ["run"])
        flags = Flags(ctx, project_flags)
        assert flags.SNOWFLAKE_PROJECTS_OTEL is True

    def test_cli_overrides_project_flags(self):
        project_flags = ProjectFlags(snowflake_projects_otel=True)
        ctx = self.make_dbt_context("run", ["--no-snowflake-projects-otel", "run"])
        flags = Flags(ctx, project_flags)
        assert flags.SNOWFLAKE_PROJECTS_OTEL is False
