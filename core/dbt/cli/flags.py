import os
import sys
from dataclasses import dataclass
from datetime import datetime
from importlib import import_module
from pathlib import Path
from pprint import pformat as pf
from typing import Any, Callable, Dict, List, Optional, Set, Union

from click import Context, Parameter, get_current_context
from click.core import Command as ClickCommand
from click.core import Group, ParameterSource

from dbt.cli.exceptions import DbtUsageException
from dbt.cli.resolvers import default_log_path, default_project_dir
from dbt.cli.types import Command as CliCommand
from dbt.config.project import read_project_flags
from dbt.contracts.project import ProjectFlags
from dbt.deprecations import fire_buffered_deprecations, renamed_env_var
from dbt.events import ALL_EVENT_NAMES
from dbt_common import ui
from dbt_common.clients import jinja
from dbt_common.events import functions
from dbt_common.exceptions import DbtInternalError
from dbt_common.helper_types import WarnErrorOptions

if os.name != "nt":
    # https://bugs.python.org/issue41567
    import multiprocessing.popen_spawn_posix  # type: ignore  # noqa: F401

FLAGS_DEFAULTS = {
    "INDIRECT_SELECTION": "eager",
    "TARGET_PATH": None,
    "DEFER_STATE": None,  # necessary because of retry construction of flags
    "WARN_ERROR": None,
    # Cli args without project_flags or env var option.
    "FULL_REFRESH": False,
    "STRICT_MODE": False,
    "STORE_FAILURES": False,
    "INTROSPECT": True,
    "STATE_MODIFIED_COMPARE_VARS": False,
}

DEPRECATED_PARAMS = {
    "deprecated_defer": "defer",
    "deprecated_favor_state": "favor_state",
    "deprecated_print": "print",
    "deprecated_state": "state",
}


WHICH_KEY = "which"


def convert_config(config_name, config_value):
    """Convert the values from config and original set_from_args to the correct type."""
    ret = config_value
    if config_name.lower() == "warn_error_options" and type(config_value) == dict:
        ret = WarnErrorOptions(
            include=config_value.get("include", []),
            exclude=config_value.get("exclude", []),
            silence=config_value.get("silence", []),
            valid_error_names=ALL_EVENT_NAMES,
        )
    return ret




def args_to_context(args: List[str]) -> Context:
    """Convert a list of args to a click context with proper hierarchy for dbt commands"""
    from dbt.cli.main import cli

    cli_ctx = cli.make_context(cli.name, args)
    # Split args if they're a comma separated string.
    if len(args) == 1 and "," in args[0]:
        args = args[0].split(",")
    sub_command_name, sub_command, args = cli.resolve_command(cli_ctx, args)
    # Handle source and docs group.
    if isinstance(sub_command, Group):
        sub_command_name, sub_command, args = sub_command.resolve_command(cli_ctx, args)

    assert isinstance(sub_command, ClickCommand)
    sub_command_ctx = sub_command.make_context(sub_command_name, args)
    sub_command_ctx.parent = cli_ctx
    return sub_command_ctx


@dataclass(frozen=True)
class Flags:
    """Primary configuration artifact for running dbt"""

    def __init__(
        self, ctx: Optional[Context] = None, project_flags: Optional[ProjectFlags] = None
    ) -> None:
        # Set the default flags.
        for key, value in FLAGS_DEFAULTS.items():
            object.__setattr__(self, key, value)
        # Use to handle duplicate params in _assign_params
        flags_defaults_list = list(FLAGS_DEFAULTS.keys())

        if ctx is None:
            ctx = get_current_context()

        def _get_params_by_source(ctx: Context, source_type: ParameterSource):
            """Generates all params of a given source type."""
            yield from [
                name for name, source in ctx._parameter_source.items() if source is source_type
            ]
            if ctx.parent:
                yield from _get_params_by_source(ctx.parent, source_type)

        # Ensure that any params sourced from the commandline are not present more than once.
        # Click handles this exclusivity, but only at a per-subcommand level.
        seen_params = []
        for param in _get_params_by_source(ctx, ParameterSource.COMMANDLINE):
            if param in seen_params:
                raise DbtUsageException(
                    f"{param.lower()} was provided both before and after the subcommand, it can only be set either before or after.",
                )
            seen_params.append(param)

        def _assign_params(
            ctx: Context,
            params_assigned_from_default: set,
            params_assigned_from_user: set,
            deprecated_env_vars: Dict[str, Callable],
        ):
            """Recursively adds all click params to flag object"""
            for param_name, param_value in ctx.params.items():
                # N.B. You have to use the base MRO method (object.__setattr__) to set attributes
                # when using frozen dataclasses.
                # https://docs.python.org/3/library/dataclasses.html#frozen-instances

                # Handle deprecated env vars while still respecting old values
                # e.g. DBT_NO_PRINT -> DBT_PRINT if DBT_NO_PRINT is set, it is
                # respected over DBT_PRINT or --print.
                new_name: Union[str, None] = None
                if param_name in DEPRECATED_PARAMS:
                    # Deprecated env vars can only be set via env var.
                    # We use the deprecated option in click to serialize the value
                    # from the env var string.
                    param_source = ctx.get_parameter_source(param_name)
                    if param_source == ParameterSource.DEFAULT:
                        continue
                    elif param_source != ParameterSource.ENVIRONMENT:
                        raise DbtUsageException(
                            "Deprecated parameters can only be set via environment variables",
                        )

                    # Rename for clarity.
                    dep_name = param_name
                    new_name = DEPRECATED_PARAMS.get(dep_name)
                    try:
                        assert isinstance(new_name, str)
                    except AssertionError:
                        raise Exception(
                            f"No deprecated param name match in DEPRECATED_PARAMS from {dep_name} to {new_name}"
                        )

                    # Find param objects for their envvar name.
                    try:
                        dep_param = [x for x in ctx.command.params if x.name == dep_name][0]
                        new_param = [x for x in ctx.command.params if x.name == new_name][0]
                    except IndexError:
                        raise Exception(
                            f"No deprecated param name match in context from {dep_name} to {new_name}"
                        )

                    # Remove param from defaulted set since the deprecated
                    # value is not set from default, but from an env var.
                    if new_name in params_assigned_from_default:
                        params_assigned_from_default.remove(new_name)

                    # Add the deprecation warning function to the set.
                    assert isinstance(dep_param.envvar, str)
                    assert isinstance(new_param.envvar, str)
                    deprecated_env_vars[new_name] = renamed_env_var(
                        old_name=dep_param.envvar,
                        new_name=new_param.envvar,
                    )
                # end deprecated_params

                # Set the flag value.
                is_duplicate = (
                    hasattr(self, param_name.upper())
                    and param_name.upper() not in flags_defaults_list
                )
                # First time through, set as though FLAGS_DEFAULTS hasn't been set, so not a duplicate.
                # Subsequent pass (to process "parent" params) should be treated as duplicates.
                if param_name.upper() in flags_defaults_list:
                    flags_defaults_list.remove(param_name.upper())
                # Note: the following determines whether parameter came from click default,
                # not from FLAGS_DEFAULTS in __init__.
                is_default = ctx.get_parameter_source(param_name) == ParameterSource.DEFAULT
                is_envvar = ctx.get_parameter_source(param_name) == ParameterSource.ENVIRONMENT

                flag_name = (new_name or param_name).upper()

                # envvar flags are assigned in either parent or child context if there
                # isn't an overriding cli command flag.
                # If the flag has been encountered as a child cli flag, we don't
                # want to overwrite with parent envvar, since the commandline flag takes precedence.
                if (is_duplicate and not (is_default or is_envvar)) or not is_duplicate:
                    object.__setattr__(self, flag_name, param_value)

                # Track default assigned params.
                # For flags that are accepted at both 'parent' and 'child' levels,
                # we need to track user-provided and default values across both,
                # to support detection of mutually exclusive flags later on.
                if not is_default:
                    params_assigned_from_user.add(param_name)
                    if param_name in params_assigned_from_default:
                        params_assigned_from_default.remove(param_name)
                if is_default and param_name not in params_assigned_from_user:
                    params_assigned_from_default.add(param_name)

            if ctx.parent:
                _assign_params(
                    ctx.parent,
                    params_assigned_from_default,
                    params_assigned_from_user,
                    deprecated_env_vars,
                )


        params_assigned_from_user = set()  # type: Set[str]
        params_assigned_from_default = set()  # type: Set[str]
        deprecated_env_vars: Dict[str, Callable] = {}
        _assign_params(
            ctx, params_assigned_from_default, params_assigned_from_user, deprecated_env_vars
        )

        # Set deprecated_env_var_warnings to be fired later after events have been init.
        object.__setattr__(
            self, "deprecated_env_var_warnings", [x for x in deprecated_env_vars.values()]
        )

        # Get the invoked command flags.
        invoked_subcommand_name = (
            ctx.invoked_subcommand if hasattr(ctx, "invoked_subcommand") else None
        )
        if invoked_subcommand_name is not None:
            invoked_subcommand = getattr(import_module("dbt.cli.main"), invoked_subcommand_name)
            invoked_subcommand.allow_extra_args = True
            invoked_subcommand.ignore_unknown_options = True
            invoked_subcommand_ctx = invoked_subcommand.make_context(None, sys.argv)
            _assign_params(
                invoked_subcommand_ctx,
                params_assigned_from_default,
                params_assigned_from_user,
                deprecated_env_vars,
            )

        if not project_flags:
            project_dir = getattr(self, "PROJECT_DIR", str(default_project_dir()))
            profiles_dir = getattr(self, "PROFILES_DIR", None)
            if profiles_dir and project_dir:
                project_flags = read_project_flags(project_dir, profiles_dir)
            else:
                project_flags = None

        # Add entire invocation command to flags
        object.__setattr__(self, "INVOCATION_COMMAND", "dbt " + " ".join(sys.argv[1:]))

        if project_flags:
            # Overwrite default assignments with project flags if available.
            param_assigned_from_default_copy = params_assigned_from_default.copy()
            for param_assigned_from_default in params_assigned_from_default:
                project_flags_param_value = getattr(
                    project_flags, param_assigned_from_default, None
                )
                if project_flags_param_value is not None:
                    object.__setattr__(
                        self,
                        param_assigned_from_default.upper(),
                        convert_config(param_assigned_from_default, project_flags_param_value),
                    )
                    param_assigned_from_default_copy.remove(param_assigned_from_default)
            params_assigned_from_default = param_assigned_from_default_copy

            # Add project-level flags that are not available as CLI options / env vars
            for (
                project_level_flag_name,
                project_level_flag_value,
            ) in project_flags.project_only_flags.items():
                object.__setattr__(self, project_level_flag_name.upper(), project_level_flag_value)

        # Set hard coded flags.
        object.__setattr__(self, "WHICH", invoked_subcommand_name or ctx.info_name)

        # Apply the lead/follow relationship between some parameters.
        self._override_if_set("USE_COLORS", "USE_COLORS_FILE", params_assigned_from_default)
        self._override_if_set("LOG_LEVEL", "LOG_LEVEL_FILE", params_assigned_from_default)
        self._override_if_set("LOG_FORMAT", "LOG_FORMAT_FILE", params_assigned_from_default)

        # Set default LOG_PATH from PROJECT_DIR, if available.
        # Starting in v1.5, if `log-path` is set in `dbt_project.yml`, it will raise a deprecation warning,
        # with the possibility of removing it in a future release.
        if getattr(self, "LOG_PATH", None) is None:
            project_dir = getattr(self, "PROJECT_DIR", str(default_project_dir()))
            version_check = getattr(self, "VERSION_CHECK", True)
            object.__setattr__(
                self, "LOG_PATH", default_log_path(Path(project_dir), version_check)
            )

        # Support console DO NOT TRACK initiative.
        if os.getenv("DO_NOT_TRACK", "").lower() in ("1", "t", "true", "y", "yes"):
            object.__setattr__(self, "SEND_ANONYMOUS_USAGE_STATS", False)

        # Check mutual exclusivity once all flags are set.
        self._assert_mutually_exclusive(
            params_assigned_from_default, ["WARN_ERROR", "WARN_ERROR_OPTIONS"]
        )

        # Handle arguments mutually exclusive with INLINE
        self._assert_mutually_exclusive(params_assigned_from_default, ["SELECT", "INLINE"])
        self._assert_mutually_exclusive(params_assigned_from_default, ["SELECTOR", "INLINE"])

        # Check event_time configs for validity
        self._validate_event_time_configs()

        # Support lower cased access for legacy code.
        params = set(
            x for x in dir(self) if not callable(getattr(self, x)) and not x.startswith("__")
        )
        for param in params:
            object.__setattr__(self, param.lower(), getattr(self, param))

        self.set_common_global_flags()

    def __str__(self) -> str:
        return str(pf(self.__dict__))

    def _override_if_set(self, lead: str, follow: str, defaulted: Set[str]) -> None:
        """If the value of the lead parameter was set explicitly, apply the value to follow, unless follow was also set explicitly."""
        if lead.lower() not in defaulted and follow.lower() in defaulted:
            object.__setattr__(self, follow.upper(), getattr(self, lead.upper(), None))
        else:
            defaulted.discard(lead.lower())

    def _assert_mutually_exclusive(
        self, params_assigned_from_default: Set[str], group: List[str]
    ) -> None:
        """
        Ensure no elements from group are simultaneously provided by a user, as inferred from params_assigned_from_default.
        Raises click.UsageError if any two elements from group are simultaneously provided by a user.
        """
        set_flag = None
        for flag in group:
            flag_set_by_user = (
                hasattr(self, flag) and flag.lower() not in params_assigned_from_default
            )
            if flag_set_by_user and set_flag:
                raise DbtUsageException(
                    f"{flag.lower()}: not allowed with argument {set_flag.lower()}"
                )
            elif flag_set_by_user:
                set_flag = flag

    def _validate_event_time_configs(self) -> None:
        event_time_start: datetime = (
            getattr(self, "EVENT_TIME_START") if hasattr(self, "EVENT_TIME_START") else None
        )
        event_time_pause: datetime = (
            getattr(self, "EVENT_TIME_PAUSE") if hasattr(self, "EVENT_TIME_PAUSE") else None
        )
        event_time_end: datetime = (
            getattr(self, "EVENT_TIME_END") if hasattr(self, "EVENT_TIME_END") else None
        )

        # only do validations if at least one of `event_time_start` or `event_time_end` are specified
        if event_time_start is not None or event_time_end is not None:

            # These `ifs`, combined with the parent `if` make it so that `event_time_start` and
            # `event_time_end` are mutually required
            if event_time_start is None:
                raise DbtUsageException(
                    "The flag `--event-time-end` was specified, but `--event-time-start` was not. "
                    "When specifying `--event-time-end`, `--event-time-start` must also be present."
                )
            
            if event_time_pause is not None:
                raise DbtUsageException(
                    "The flag `--event-time-pause` is not supported when `--event-time-start` or `--event-time-end` are specified. "
                    "Please remove `--event-time-pause` from the command.")

            if event_time_end is None:
                raise DbtUsageException(
                    "The flag `--event-time-start` was specified, but `--event-time-end` was not. "
                    "When specifying `--event-time-start`, `--event-time-end` must also be present."
                )

            # This `if` just is a sanity check that `event_time_start` is before `event_time_end`
            if event_time_start >= event_time_end:
                raise DbtUsageException(
                    "Value for `--event-time-start` must be less than `--event-time-end`"
                )

    def fire_deprecations(self):
        """Fires events for deprecated env_var usage."""
        [dep_fn() for dep_fn in self.deprecated_env_var_warnings]
        # It is necessary to remove this attr from the class so it does
        # not get pickled when written to disk as json.
        object.__delattr__(self, "deprecated_env_var_warnings")

        fire_buffered_deprecations()

    @classmethod
    def from_dict(cls, command: CliCommand, args_dict: Dict[str, Any]) -> "Flags":
        command_arg_list = command_params(command, args_dict)
        ctx = args_to_context(command_arg_list)
        flags = cls(ctx=ctx)
        flags.fire_deprecations()
        return flags

    def set_common_global_flags(self):
        # Set globals for common.ui
        if getattr(self, "PRINTER_WIDTH", None) is not None:
            ui.PRINTER_WIDTH = getattr(self, "PRINTER_WIDTH")
        if getattr(self, "USE_COLORS", None) is not None:
            ui.USE_COLOR = getattr(self, "USE_COLORS")

        # Set globals for common.events.functions
        functions.WARN_ERROR = getattr(self, "WARN_ERROR", False)
        if getattr(self, "WARN_ERROR_OPTIONS", None) is not None:
            functions.WARN_ERROR_OPTIONS = getattr(self, "WARN_ERROR_OPTIONS")

        

        # Set globals for common.jinja
        if getattr(self, "MACRO_DEBUGGING", None) is not None:
            jinja.MACRO_DEBUGGING = getattr(self, "MACRO_DEBUGGING")

    # This is here to prevent mypy from complaining about all of the
    # attributes which we added dynamically.
    def __getattr__(self, name: str) -> Any:
        return super().__getattribute__(name)  # type: ignore


CommandParams = List[str]


def command_params(command: CliCommand, args_dict: Dict[str, Any]) -> CommandParams:
    """Given a command and a dict, returns a list of strings representing
    the CLI params for that command. The order of this list is consistent with
    which flags are expected at the parent level vs the command level.

    e.g. fn("run", {"defer": True, "print": False}) -> ["--no-print", "run", "--defer"]

    The result of this function can be passed in to the args_to_context function
    to produce a click context to instantiate Flags with.
    """

    cmd_args = set(command_args(command))
    prnt_args = set(parent_args())
    default_args = set([x.lower() for x in FLAGS_DEFAULTS.keys()])

    res = command.to_list()
    for k, v in args_dict.items():
        k = k.lower()
        # if a "which" value exists in the args dict, it should match the command provided
        if k == WHICH_KEY:
            if v != command.value:
                raise DbtInternalError(
                    f"Command '{command.value}' does not match value of which: '{v}'"
                )
            continue

        # param was assigned from defaults and should not be included
        if k not in (cmd_args | prnt_args) or (
            k in default_args and v == FLAGS_DEFAULTS[k.upper()]
        ):
            continue

        # if the param is in parent args, it should come before the arg name
        # e.g. ["--print", "run"] vs ["run", "--print"]
        add_fn = res.append
        if k in prnt_args:

            def add_fn(x):
                res.insert(0, x)

        spinal_cased = k.replace("_", "-")

        # MultiOption flags come back as lists, but we want to pass them as space separated strings
        if isinstance(v, list):
            if len(v) > 0:
                v = " ".join(v)
            else:
                continue

        if k == "macro" and command == CliCommand.RUN_OPERATION:
            add_fn(v)
        # None is a Singleton, False is a Flyweight, only one instance of each.
        elif (v is None or v is False) and k not in (
            # These are None by default but they do not support --no-{flag}
            "defer_state",
            "log_format",
        ):
            add_fn(f"--no-{spinal_cased}")
        elif v is True:
            add_fn(f"--{spinal_cased}")
        else:
            add_fn(f"--{spinal_cased}={v}")

    return res


ArgsList = List[str]


def parent_args() -> ArgsList:
    """Return a list representing the params the base click command takes."""
    from dbt.cli.main import cli

    return format_params(cli.params)


def command_args(command: CliCommand) -> ArgsList:
    """Given a command, return a list of strings representing the params
    that command takes. This function only returns params assigned to a
    specific command, not those of its parent command.

    e.g. fn("run") -> ["defer", "favor_state", "exclude", ...]
    """
    import dbt.cli.main as cli

    CMD_DICT: Dict[CliCommand, ClickCommand] = {
        CliCommand.BUILD: cli.build,
        CliCommand.CLEAN: cli.clean,
        CliCommand.CLONE: cli.clone,
        CliCommand.COMPILE: cli.compile,
        CliCommand.DOCS_GENERATE: cli.docs_generate,
        CliCommand.DOCS_SERVE: cli.docs_serve,
        CliCommand.DEBUG: cli.debug,
        CliCommand.DEPS: cli.deps,
        CliCommand.INIT: cli.init,
        CliCommand.LIST: cli.list,
        CliCommand.PARSE: cli.parse,
        CliCommand.RUN: cli.run,
        CliCommand.RUN_OPERATION: cli.run_operation,
        CliCommand.SEED: cli.seed,
        CliCommand.SHOW: cli.show,
        CliCommand.SNAPSHOT: cli.snapshot,
        CliCommand.SOURCE_FRESHNESS: cli.freshness,
        CliCommand.TEST: cli.test,
        CliCommand.RETRY: cli.retry,
    }
    click_cmd: Optional[ClickCommand] = CMD_DICT.get(command, None)
    if click_cmd is None:
        raise DbtInternalError(f"No command found for name '{command.name}'")
    return format_params(click_cmd.params)


def format_params(params: List[Parameter]) -> ArgsList:
    return [str(x.name) for x in params if not str(x.name).lower().startswith("deprecated_")]
