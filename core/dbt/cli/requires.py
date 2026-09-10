import importlib.util
import os
import time
import traceback
from functools import update_wrapper
from typing import Dict, Optional

from click import Context

import dbt.tracking
from dbt.adapters.capability import Capability
from dbt.adapters.factory import adapter_management, get_adapter, register_adapter
from dbt.cli.exceptions import ExceptionExit, ResultExit
from dbt.cli.flags import Flags
from dbt.config import RuntimeConfig
from dbt.config.catalogs import (
    get_active_write_integration,
    load_catalogs,
    load_catalogs_v2,
)
from dbt.config.runtime import UnsetProfile, load_profile, load_project
from dbt.context.providers import generate_runtime_macro_context
from dbt.context.query_header import generate_query_header_context
from dbt.deprecated_version import check_deprecated_version
from dbt.deprecations import show_deprecations_summary
from dbt.env_vars import KNOWN_ENGINE_ENV_VARS, validate_engine_env_vars
from dbt.events.logging import setup_event_logger
from dbt.events.types import (
    ArtifactUploadError,
    CommandCompleted,
    MainEncounteredError,
    MainReportArgs,
    MainReportVersion,
    MainStackTrace,
    MainTrackingUserState,
    ResourceReport,
)
from dbt.exceptions import DbtProjectError, FailFastError
from dbt.flags import get_flag_dict, get_flags, set_flags
from dbt.mp_context import get_mp_context
from dbt.parser.manifest import parse_manifest
from dbt.plugins import set_up_plugin_manager
from dbt.profiler import profiler
from dbt.tracking import initialize_from_flags, track_manage_state, track_run
from dbt.utils import try_get_max_rss_kb
from dbt.utils.artifact_upload import upload_artifacts
from dbt.version import installed as installed_version
from dbt_common.clients.system import get_env
from dbt_common.context import get_invocation_context, set_invocation_context
from dbt_common.dataclass_schema import ValidationError
from dbt_common.events.base_types import EventGroupType, EventLevel
from dbt_common.events.event_manager_client import get_event_manager
from dbt_common.events.functions import LOG_VERSION, fire_deferred_events, fire_event
from dbt_common.events.helpers import get_json_string_utcnow
from dbt_common.events.types import Note
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


def _cross_propagate_engine_env_vars(env_dict: Dict[str, str]) -> None:
    for env_var in KNOWN_ENGINE_ENV_VARS:
        if env_var.old_name is not None:
            # If the old name is in the env dict, and not the new name, set the new name based on the old name
            if env_var.old_name in env_dict and env_var.name not in env_dict:
                env_dict[env_var.name] = env_dict[env_var.old_name]
            # If the new name is in the env dict, override the old name with it
            elif env_var.name in env_dict:
                env_dict[env_var.old_name] = env_dict[env_var.name]


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
        env_dict = get_env()
        _cross_propagate_engine_env_vars(env_dict)
        get_invocation_context()._env = env_dict

        # Flags
        flags = Flags(ctx)
        ctx.obj["flags"] = flags
        set_flags(flags)
        get_invocation_context().enable_snowflake_projects_otel = getattr(
            flags, "SNOWFLAKE_PROJECTS_OTEL", False
        )
        get_event_manager().require_warn_or_error_handling = (
            flags.require_all_warnings_handled_by_warn_error
        )

        # Reset invocation_id for each 'invocation' of a dbt command (can happen multiple times in a single process)
        reset_invocation_id()

        # Logging
        callbacks = ctx.obj.get("callbacks", [])
        setup_event_logger(flags=flags, callbacks=callbacks)
        get_event_manager().allow_deferral = flags.enable_grouped_warn_error_parser_logs

        # Tracking
        initialize_from_flags(flags.SEND_ANONYMOUS_USAGE_STATS, flags.PROFILES_DIR)
        ctx.with_resource(track_run(run_command=flags.WHICH))

        # Telemetry for opt-in state management. Source is attributed to whichever
        # surface enabled it: --manage-state (cli_flag), DBT_ENGINE_MANAGE_STATE
        # (env_var), dbtRunner.invoke(manage_state=...) (programmatic),
        # manage_state: true in dbt_project.yml (project_config), or
        # user_settings.yml (user_settings).
        if getattr(flags, "MANAGE_STATE", False) and dbt.tracking.active_user is not None:
            track_manage_state(
                {
                    "manage_state": True,
                    "source": getattr(flags, "MANAGE_STATE_SOURCE", None),
                }
            )

        # Now that we have our logger, fire away!
        fire_event(MainReportVersion(version=str(installed_version), log_version=LOG_VERSION))
        flags_dict_str = cast_dict_to_dict_of_strings(get_flag_dict())
        fire_event(MainReportArgs(args=flags_dict_str))

        # `init` already fires its own WARNING-level version of this notice
        if flags.WHICH != "init":
            check_deprecated_version()

        # Deprecation warnings
        flags.fire_deprecations(ctx)

        if dbt.tracking.active_user is not None:  # mypy appeasement, always true
            fire_event(MainTrackingUserState(user_state=dbt.tracking.active_user.state()))

        # Profiling
        if flags.RECORD_TIMING_INFO:
            ctx.with_resource(profiler(enable=True, outfile=flags.RECORD_TIMING_INFO))

        # Adapter management
        ctx.with_resource(adapter_management())

        # Validate engine env var restricted name space
        validate_engine_env_vars()

        return func(*args, **kwargs)

    return update_wrapper(wrapper, func)


def setup_record_replay():
    rec_mode = get_record_mode_from_env()
    rec_types = get_record_types_from_env()

    recorder: Optional[Recorder] = None
    if rec_mode == RecorderMode.REPLAY:
        previous_recording_path = os.environ.get(
            "DBT_ENGINE_RECORDER_FILE_PATH"
        ) or os.environ.get("DBT_RECORDER_FILE_PATH")
        recorder = Recorder(
            RecorderMode.REPLAY, types=rec_types, previous_recording_path=previous_recording_path
        )
    elif rec_mode == RecorderMode.DIFF:
        previous_recording_path = os.environ.get(
            "DBT_ENGINE_RECORDER_FILE_PATH"
        ) or os.environ.get("DBT_RECORDER_FILE_PATH")
        # ensure types match the previous recording
        types = get_record_types_from_dict(previous_recording_path)
        recorder = Recorder(
            RecorderMode.DIFF, types=types, previous_recording_path=previous_recording_path
        )
    elif rec_mode == RecorderMode.RECORD:
        recorder = Recorder(RecorderMode.RECORD, types=rec_types)

    get_invocation_context().recorder = recorder


def tear_down_record_replay():
    recorder = get_invocation_context().recorder
    if recorder is not None:
        if recorder.mode == RecorderMode.RECORD:
            recorder.write()
        if recorder.mode == RecorderMode.DIFF:
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
        except ValidationError as e:
            fire_event(MainEncounteredError(exc=str(e)))
            raise ExceptionExit(e)
        except BaseException as e:
            fire_event(MainEncounteredError(exc=str(e)))
            fire_event(MainStackTrace(stack_trace=traceback.format_exc()))
            raise ExceptionExit(e)
        finally:
            # Fire ResourceReport, but only on systems which support the resource
            # module. (Skip it on Windows).
            try:
                if get_flags().upload_to_artifacts_ingest_api:
                    upload_artifacts(
                        get_flags().project_dir, get_flags().target_path, ctx.command.name
                    )

            except Exception as e:
                fire_event(ArtifactUploadError(msg=str(e)))

            show_deprecations_summary()

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
        get_invocation_context().uses_adapter(profile.credentials.type)

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
        # TODO deprecations warnings fired from loading the project will lack
        # the project_id in the snowplow event.

        # Determine if vars should be required during project loading.
        # Commands that don't need vars evaluated (like 'deps', 'clean')
        # should use lenient mode (require_vars=False) to allow missing vars.
        # Commands that validate or execute (like 'run', 'compile', 'build', 'debug') should use
        # strict mode (require_vars=True) to show helpful "Required var X not found" errors.
        # If adding more commands to lenient mode, update this condition.
        require_vars = flags.WHICH != "deps"

        project = load_project(
            flags.PROJECT_DIR,
            flags.VERSION_CHECK,
            ctx.obj["profile"],
            flags.VARS,
            validate=True,
            require_vars=require_vars,
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
    given a flags object, profile, project, and runtime config. This also registers the adapter
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


def _bridge_v2_catalogs(runtime_config: "RuntimeConfig", catalogs_v2: list) -> list:
    # Bridge requires an adapter instance for type() and override hooks.
    # parse_manifest will register the adapter again (same config); that
    # second registration is harmless and ensures integrations are present
    # when ManifestLoader runs catalog lookups during parsing.
    register_adapter(runtime_config, get_mp_context())
    adapter = get_adapter(runtime_config)
    _cat_v2 = getattr(Capability, "CatalogsV2", None)  # type: ignore[attr-defined]
    if _cat_v2 is None or not adapter.capabilities()[_cat_v2]:
        raise DbtProjectError(
            f"Adapter '{adapter.type()}' does not support catalogs.yml v2 yet. "
            f"Use catalogs.yml v1 or upgrade to a supported adapter version."
        )
    return [adapter.bridge_v2_catalog(c) for c in catalogs_v2]


def setup_manifest(ctx: Context, write: bool = True, write_perf_info: bool = False):
    """Load the manifest and add it to the context."""
    req_strs = ["flags", "profile", "project", "runtime_config"]
    reqs = [ctx.obj.get(dep) for dep in req_strs]

    if None in reqs:
        raise DbtProjectError("flags, profile, project, and runtime_config required for manifest")

    runtime_config = ctx.obj["runtime_config"]

    flags = ctx.obj["flags"]
    project_name = ctx.obj["project"].project_name
    use_v2 = flags.USE_CATALOGS_V2

    if use_v2:
        catalogs_v2 = load_catalogs_v2(flags.PROJECT_DIR, project_name, flags.VARS)
        # TODO: ctx.obj["catalogs"] is List[CatalogV2] here but downstream tasks
        # (base.py, build.py, run.py, etc.) annotate it as Optional[List[Catalog]].
        # Widen signatures to Sequence[Union[Catalog, CatalogV2]] or add a common base.
        ctx.obj["catalogs"] = catalogs_v2
        if catalogs_v2:
            fire_event(
                Note(
                    msg="catalogs.yml v2 schema validation is experimental, not officially supported yet, "
                    "and its spec is liable to change. "
                    "See https://github.com/dbt-labs/dbt-core/discussions/12723"
                ),
                EventLevel.WARN,
            )
            active_integrations = _bridge_v2_catalogs(runtime_config, catalogs_v2)
        else:
            active_integrations = []
    else:
        catalogs = load_catalogs(flags.PROJECT_DIR, project_name, flags.VARS)
        active_integrations = [get_active_write_integration(catalog) for catalog in catalogs]
        ctx.obj["catalogs"] = catalogs

    # if a manifest has already been set on the context, don't overwrite it
    if ctx.obj.get("manifest") is None:
        if getattr(flags, "USE_V2_PARSER", False):
            from dbt.parser.v2 import parse_with_v2

            ctx.obj["manifest"] = parse_with_v2(runtime_config, write, ctx.obj["flags"].write_json)
            _wire_adapter_for_external_manifest(
                runtime_config, ctx.obj["manifest"], active_integrations
            )
        else:
            ctx.obj["manifest"] = parse_manifest(
                runtime_config,
                write_perf_info,
                write,
                ctx.obj["flags"].write_json,
                active_integrations,
            )
    else:
        _wire_adapter_for_external_manifest(
            runtime_config, ctx.obj["manifest"], active_integrations
        )

    fire_deferred_events(event_group_type=EventGroupType.PARSE)


def _wire_adapter_for_external_manifest(runtime_config, manifest, active_integrations):
    """Register and configure the adapter for a manifest that was produced
    outside of parse_manifest() — e.g. pre-set on the context, or loaded
    from the v2 parser.
    """
    register_adapter(runtime_config, get_mp_context())
    adapter = get_adapter(runtime_config)
    adapter.set_macro_context_generator(generate_runtime_macro_context)  # type: ignore[arg-type]
    adapter.set_macro_resolver(manifest)
    query_header_context = generate_query_header_context(adapter.config, manifest)  # type: ignore[attr-defined]
    adapter.connections.set_query_header(query_header_context)
    for integration in active_integrations:
        adapter.add_catalog_integration(integration)
    return adapter
