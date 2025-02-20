import importlib.util
import os
import time
import traceback
from functools import update_wrapper
from typing import Optional

from click import Context

import dbt.tracking
from dbt.adapters.factory import adapter_management, get_adapter, register_adapter
from dbt.cli.exceptions import ExceptionExit, ResultExit
from dbt.cli.flags import Flags
from dbt.config import RuntimeConfig
from dbt.config.runtime import UnsetProfile, load_profile, load_project
from dbt.context.providers import generate_runtime_macro_context
from dbt.context.query_header import generate_query_header_context
from dbt.events.logging import setup_event_logger
from dbt.events.types import (
    CommandCompleted,
    MainEncounteredError,
    MainReportArgs,
    MainReportVersion,
    MainStackTrace,
    MainTrackingUserState,
    ResourceReport,
)
from dbt.exceptions import DbtProjectError, FailFastError
from dbt.flags import get_flag_dict, set_flags
from dbt.mp_context import get_mp_context
from dbt.parser.manifest import parse_manifest
from dbt.plugins import set_up_plugin_manager
from dbt.profiler import profiler
from dbt.tracking import active_user, initialize_from_flags, track_run
from dbt.utils import try_get_max_rss_kb
from dbt.version import installed as installed_version
from dbt_common.clients.system import get_env
from dbt_common.context import get_invocation_context, set_invocation_context
from dbt_common.events.base_types import EventLevel
from dbt_common.events.functions import LOG_VERSION, fire_event
from dbt_common.events.helpers import get_json_string_utcnow
from dbt_common.exceptions import DbtBaseException as DbtException
from dbt_common.invocation import reset_invocation_id
from dbt_common.record import (
    Recorder,
    RecorderMode,
    get_record_mode_from_env,
    get_record_types_from_dict,
    get_record_types_from_env,
)
from dbt_common.utils import cast_dict_to_dict_of_strings


def preflight(func):
    def wrapper(*args, **kwargs):
        ctx = args[0]
        assert isinstance(ctx, Context)
        ctx.obj = ctx.obj or {}

        set_invocation_context({})

        # Record/Replay
        setup_record_replay()

        # Must be set after record/replay is set up so that the env can be
        # recorded or replayed if needed.
        get_invocation_context()._env = get_env()

        # Flags
        flags = Flags(ctx)
        ctx.obj["flags"] = flags
        set_flags(flags)

        # Reset invocation_id for each 'invocation' of a dbt command (can happen multiple times in a single process)
        reset_invocation_id()

        # Logging
        callbacks = ctx.obj.get("callbacks", [])
        setup_event_logger(flags=flags, callbacks=callbacks)

        # Tracking
        initialize_from_flags(flags.SEND_ANONYMOUS_USAGE_STATS, flags.PROFILES_DIR)
        ctx.with_resource(track_run(run_command=flags.WHICH))

        # Now that we have our logger, fire away!
        fire_event(MainReportVersion(version=str(installed_version), log_version=LOG_VERSION))
        flags_dict_str = cast_dict_to_dict_of_strings(get_flag_dict())
        fire_event(MainReportArgs(args=flags_dict_str))

        # Deprecation warnings
        flags.fire_deprecations()

        if active_user is not None:  # mypy appeasement, always true
            fire_event(MainTrackingUserState(user_state=active_user.state()))

        # Profiling
        if flags.RECORD_TIMING_INFO:
            ctx.with_resource(profiler(enable=True, outfile=flags.RECORD_TIMING_INFO))

        # Adapter management
        ctx.with_resource(adapter_management())

        return func(*args, **kwargs)

    return update_wrapper(wrapper, func)


def setup_record_replay():
    rec_mode = get_record_mode_from_env()
    rec_types = get_record_types_from_env()

    recorder: Optional[Recorder] = None
    if rec_mode == RecorderMode.REPLAY:
        previous_recording_path = os.environ.get("DBT_RECORDER_FILE_PATH")
        recorder = Recorder(
            RecorderMode.REPLAY, types=rec_types, previous_recording_path=previous_recording_path
        )
    elif rec_mode == RecorderMode.DIFF:
        previous_recording_path = os.environ.get("DBT_RECORDER_FILE_PATH")
        # ensure types match the previous recording
        types = get_record_types_from_dict(previous_recording_path)
        recorder = Recorder(
            RecorderMode.DIFF, types=types, previous_recording_path=previous_recording_path
        )
    elif rec_mode == RecorderMode.RECORD:
        recorder = Recorder(RecorderMode.RECORD, types=rec_types)

    get_invocation_context().recorder = recorder


 def tear_down_record_play():
    recorder = get_invocation_context().recorder
    if recorder is recorder
        if recorder.mode == RecorderMode.RECORD:
            recorder.write()


def tear_down_record_replay():
    recorder = get_invocation_context().recorder
    if recorder is not None:
        if recorder.mode == RecorderMode.RECORD:
            recorder.write()
        if recorder.mode == RecorderMode.DIFF:
            recorder.write()
        if recorder.mode == RecorderMode.RECORD:
            recorder.write()
        if recorder.mode ==  RecorderMode.DIFF:
            recorder.write()
            recorder.write_diffs(diff_file_name="recording_diffs.json")
        elif recorder.mode == RecorderMode.REPLAY:
            recorder.write_diffs("replay_diffs.json")

        


def postflight(func):
    """The decorator that handles all exception handling for the click commands.
    This decorator must be used before any other decorators that may throw an exception."""

    def wrapper(*args, **kwargs):
        ctx = args[0]
        start_func = time.perf_counter()
        success = False

        try:
            result, success = func(*args, **kwargs)
        except FailFastError as e:
            fire_event(MainEncounteredError(exc=str(e)))
            raise ResultExit(e.result)
        except DbtException as e:
            fire_event(MainEncounteredError(exc=str(e)))
            raise ExceptionExit(e)
        except BaseException as e:
            fire_event(MainEncounteredError(exc=str(e)))
            fire_event(MainStackTrace(stack_trace=traceback.format_exc()))
            raise ExceptionExit(e)
        finally:
            # Fire ResourceReport, but only on systems which support the resource
            # module. (Skip it on Windows).
            if importlib.util.find_spec("resource") is not None:
                import resource

                rusage = resource.getrusage(resource.RUSAGE_SELF)
                fire_event(
                    ResourceReport(
                        command_name=ctx.command.name,
                        command_success=success,
                        command_wall_clock_time=time.perf_counter() - start_func,
                        process_user_time=rusage.ru_utime,
                        process_kernel_time=rusage.ru_stime,
                        process_mem_max_rss=try_get_max_rss_kb() or rusage.ru_maxrss,
                        process_in_blocks=rusage.ru_inblock,
                        process_out_blocks=rusage.ru_oublock,
                    ),
                    (
                        EventLevel.INFO
                        if "flags" in ctx.obj and ctx.obj["flags"].SHOW_RESOURCE_REPORT
                        else None
                    ),
                )

            fire_event(
                CommandCompleted(
                    command=ctx.command_path,
                    success=success,
                    completed_at=get_json_string_utcnow(),
                    elapsed=time.perf_counter() - start_func,
                )
            )

            tear_down_record_replay()

        if not success:
            raise ResultExit(result)

        return (result, success)

    return update_wrapper(wrapper, func)


# TODO: UnsetProfile is necessary for deps and clean to load a project.
# This decorator and its usage can be removed once https://github.com/dbt-labs/dbt-core/issues/6257 is closed.
def unset_profile(func):
    def wrapper(*args, **kwargs):
        ctx = args[0]
        assert isinstance(ctx, Context)

        profile = UnsetProfile()
        ctx.obj["profile"] = profile

        return func(*args, **kwargs)

    return update_wrapper(wrapper, func)


def profile(func):
    def wrapper(*args, **kwargs):
        ctx = args[0]
        assert isinstance(ctx, Context)

        flags = ctx.obj["flags"]
        # TODO: Generalize safe access to flags.THREADS:
        # https://github.com/dbt-labs/dbt-core/issues/6259
        threads = getattr(flags, "THREADS", None)
        profile = load_profile(flags.PROJECT_DIR, flags.VARS, flags.PROFILE, flags.TARGET, threads)
        ctx.obj["profile"] = profile

        return func(*args, **kwargs)

    return update_wrapper(wrapper, func)


def project(func):
    def wrapper(*args, **kwargs):
        ctx = args[0]
        assert isinstance(ctx, Context)

        # TODO: Decouple target from profile, and remove the need for profile here:
        # https://github.com/dbt-labs/dbt-core/issues/6257
        if not ctx.obj.get("profile"):
            raise DbtProjectError("profile required for project")

        flags = ctx.obj["flags"]
        project = load_project(
            flags.PROJECT_DIR, flags.VERSION_CHECK, ctx.obj["profile"], flags.VARS
        )
        ctx.obj["project"] = project

        # Plugins
        set_up_plugin_manager(project_name=project.project_name)

        if dbt.tracking.active_user is not None:
            project_id = None if project is None else project.hashed_name()

            dbt.tracking.track_project_id({"project_id": project_id})

        return func(*args, **kwargs)

    return update_wrapper(wrapper, func)


def runtime_config(func):
    """A decorator used by click command functions for generating a runtime
    config given a profile and project.
    """

    def wrapper(*args, **kwargs):
        ctx = args[0]
        assert isinstance(ctx, Context)

        req_strs = ["profile", "project"]
        reqs = [ctx.obj.get(req_str) for req_str in req_strs]

        if None in reqs:
            raise DbtProjectError("profile and project required for runtime_config")

        config = RuntimeConfig.from_parts(
            ctx.obj["project"],
            ctx.obj["profile"],
            ctx.obj["flags"],
        )

        ctx.obj["runtime_config"] = config

        if dbt.tracking.active_user is not None:
            adapter_type = (
                getattr(config.credentials, "type", None)
                if hasattr(config, "credentials")
                else None
            )
            adapter_unique_id = (
                config.credentials.hashed_unique_field()
                if hasattr(config, "credentials")
                else None
            )

            dbt.tracking.track_adapter_info(
                {
                    "adapter_type": adapter_type,
                    "adapter_unique_id": adapter_unique_id,
                }
            )

        return func(*args, **kwargs)

    return update_wrapper(wrapper, func)


def manifest(*args0, write=True, write_perf_info=False):
    """A decorator used by click command functions for generating a manifest
    given a profile, project, and runtime config. This also registers the adapter
    from the runtime config and conditionally writes the manifest to disk.
    """

    def outer_wrapper(func):
        def wrapper(*args, **kwargs):
            ctx = args[0]
            assert isinstance(ctx, Context)

            setup_manifest(ctx, write=write, write_perf_info=write_perf_info)
            return func(*args, **kwargs)

        return update_wrapper(wrapper, func)

    # if there are no args, the decorator was used without params @decorator
    # otherwise, the decorator was called with params @decorator(arg)
    if len(args0) == 0:
        return outer_wrapper
    return outer_wrapper(args0[0])


def setup_manifest(ctx: Context, write: bool = True, write_perf_info: bool = False):
    """Load the manifest and add it to the context."""
    req_strs = ["profile", "project", "runtime_config"]
    reqs = [ctx.obj.get(dep) for dep in req_strs]

    if None in reqs:
        raise DbtProjectError("profile, project, and runtime_config required for manifest")

    runtime_config = ctx.obj["runtime_config"]

    # if a manifest has already been set on the context, don't overwrite it
    if ctx.obj.get("manifest") is None:
        ctx.obj["manifest"] = parse_manifest(
            runtime_config, write_perf_info, write, ctx.obj["flags"].write_json
        )
    else:
        register_adapter(runtime_config, get_mp_context())
        adapter = get_adapter(runtime_config)
        adapter.set_macro_context_generator(generate_runtime_macro_context)  # type: ignore[arg-type]
        adapter.set_macro_resolver(ctx.obj["manifest"])
        query_header_context = generate_query_header_context(adapter.config, ctx.obj["manifest"])  # type: ignore[attr-defined]
        adapter.connections.set_query_header(query_header_context)
