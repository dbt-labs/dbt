import os
import threading
import time
import traceback
from abc import ABCMeta, abstractmethod
from contextlib import nullcontext
from datetime import timezone, datetime
from pathlib import Path
from typing import Any, Dict, List, Optional, Set

from dbt.compilation import Compiler
import dbt_common.exceptions.base
import dbt.exceptions
from dbt import tracking
from dbt.cli.flags import Flags
from dbt.config import RuntimeConfig
from dbt.config.profile import read_profile
from dbt.constants import DBT_PROJECT_FILE_NAME
from dbt.contracts.graph.manifest import Manifest
from dbt.artifacts.resources.types import NodeType
from dbt.artifacts.schemas.results import TimingInfo, collect_timing_info
from dbt.artifacts.schemas.results import NodeStatus, RunningStatus, RunStatus
from dbt.artifacts.schemas.run import RunResult
from dbt_common.events.contextvars import get_node_info
from dbt_common.events.functions import fire_event
from dbt.events.types import (
    SkippingDetails,
    NodeCompiling,
    NodeExecuting,
    CatchableExceptionOnRun,
    InternalErrorOnRun,
    GenericExceptionOnRun,
    NodeConnectionReleaseError,
    LogDebugStackTrace,
    LogSkipBecauseError,
)
from dbt_common.exceptions import (
    DbtRuntimeError,
    DbtInternalError,
    CompilationError,
    NotImplementedError,
)
from dbt.events.types import (
    LogDbtProjectError,
    LogDbtProfileError,
)
from dbt.flags import get_flags
from dbt.graph import Graph
from dbt.logger import log_manager
from dbt.task.printer import print_run_result_error


def read_profiles(profiles_dir=None):
    """This is only used for some error handling"""
    if profiles_dir is None:
        profiles_dir = get_flags().PROFILES_DIR

    raw_profiles = read_profile(profiles_dir)

    if raw_profiles is None:
        profiles = {}
    else:
        profiles = {k: v for (k, v) in raw_profiles.items() if k != "config"}

    return profiles


class BaseTask(metaclass=ABCMeta):
    def __init__(self, args: Flags) -> None:
        self.args = args

    @classmethod
    def pre_init_hook(cls, args: Flags):
        """A hook called before the task is initialized."""
        if args.log_format == "json":
            log_manager.format_json()
        else:
            log_manager.format_text()

    @classmethod
    def set_log_format(cls):
        if get_flags().LOG_FORMAT == "json":
            log_manager.format_json()
        else:
            log_manager.format_text()

    @abstractmethod
    def run(self):
        raise dbt_common.exceptions.base.NotImplementedError("Not Implemented")

    def interpret_results(self, results):
        return True


def get_nearest_project_dir(project_dir: Optional[str]) -> Path:
    # If the user provides an explicit project directory, use that
    # but don't look at parent directories.
    if project_dir:
        cur_dir = Path(project_dir)
        project_file = Path(project_dir) / DBT_PROJECT_FILE_NAME
        if project_file.is_file():
            return cur_dir
        else:
            raise dbt_common.exceptions.DbtRuntimeError(
                "fatal: Invalid --project-dir flag. Not a dbt project. "
                "Missing dbt_project.yml file"
            )

    cur_dir = Path.cwd()
    project_file = cur_dir / DBT_PROJECT_FILE_NAME
    if project_file.is_file():
        return cur_dir
    else:
        raise dbt_common.exceptions.DbtRuntimeError(
            "fatal: Not a dbt project (or any of the parent directories). "
            "Missing dbt_project.yml file"
        )


def move_to_nearest_project_dir(project_dir: Optional[str]) -> Path:
    nearest_project_dir = get_nearest_project_dir(project_dir)
    os.chdir(nearest_project_dir)
    return nearest_project_dir


# TODO: look into deprecating this class in favor of several small functions that
# produce the same behavior. currently this class only contains manifest compilation,
# holding a manifest, and moving direcories.
class ConfiguredTask(BaseTask):
    def __init__(
        self, args: Flags, config: RuntimeConfig, manifest: Optional[Manifest] = None
    ) -> None:
        super().__init__(args)
        self.config = config
        self.graph: Optional[Graph] = None
        self.manifest = manifest
        self.compiler = Compiler(self.config)

    def compile_manifest(self):
        if self.manifest is None:
            raise DbtInternalError("compile_manifest called before manifest was loaded")

        start_compile_manifest = time.perf_counter()

        self.graph = self.compiler.compile(self.manifest)

        compile_time = time.perf_counter() - start_compile_manifest
        if dbt.tracking.active_user is not None:
            dbt.tracking.track_runnable_timing({"graph_compilation_elapsed": compile_time})

    @classmethod
    def from_args(cls, args: Flags, *pargs, **kwargs):
        move_to_nearest_project_dir(args.project_dir)
        try:
            # This is usually RuntimeConfig
            config = RuntimeConfig.from_args(args)
        except dbt.exceptions.DbtProjectError as exc:
            fire_event(LogDbtProjectError(exc=str(exc)))

            tracking.track_invalid_invocation(args=args, result_type=exc.result_type)
            raise dbt_common.exceptions.DbtRuntimeError("Could not run dbt") from exc
        except dbt.exceptions.DbtProfileError as exc:
            all_profile_names = list(read_profiles(get_flags().PROFILES_DIR).keys())
            fire_event(LogDbtProfileError(exc=str(exc), profiles=all_profile_names))
            tracking.track_invalid_invocation(args=args, result_type=exc.result_type)
            raise dbt_common.exceptions.DbtRuntimeError("Could not run dbt") from exc
        return cls(args, config, *pargs, **kwargs)


class ExecutionContext:
    """During execution and error handling, dbt makes use of mutable state:
    timing information and the newest (compiled vs executed) form of the node.
    """

    def __init__(self, node) -> None:
        self.timing: List[TimingInfo] = []
        self.node = node


class BaseRunner(metaclass=ABCMeta):
    def __init__(self, config, adapter, node, node_index, num_nodes) -> None:
        self.config = config
        self.compiler = Compiler(config)
        self.adapter = adapter
        self.node = node
        self.node_index = node_index
        self.num_nodes = num_nodes

        self.skip = False
        self.skip_cause: Optional[RunResult] = None

        self.run_ephemeral_models = False

    @abstractmethod
    def compile(self, manifest: Manifest) -> Any:
        pass

    def _node_build_path(self) -> Optional[str]:
        return self.node.build_path if hasattr(self.node, "build_path") else None

    def get_result_status(self, result) -> Dict[str, str]:
        if result.status == NodeStatus.Error:
            return {"node_status": "error", "node_error": str(result.message)}
        elif result.status == NodeStatus.Skipped:
            return {"node_status": "skipped"}
        elif result.status == NodeStatus.Fail:
            return {"node_status": "failed"}
        elif result.status == NodeStatus.Warn:
            return {"node_status": "warn"}
        else:
            return {"node_status": "passed"}

    def run_with_hooks(self, manifest):
        if self.skip:
            return self.on_skip()

        # no before/after printing for ephemeral mdoels
        if not self.node.is_ephemeral_model:
            self.before_execute()

        result = self.safe_run(manifest)
        self.node.update_event_status(
            node_status=result.status,
            finished_at=datetime.now(timezone.utc).replace(tzinfo=None).isoformat(),
        )

        if not self.node.is_ephemeral_model:
            self.after_execute(result)

        return result

    def _build_run_result(
        self,
        node,
        start_time,
        status,
        timing_info,
        message,
        agate_table=None,
        adapter_response=None,
        failures=None,
    ):
        execution_time = time.time() - start_time
        thread_id = threading.current_thread().name
        if adapter_response is None:
            adapter_response = {}
        return RunResult(
            status=status,
            thread_id=thread_id,
            execution_time=execution_time,
            timing=timing_info,
            message=message,
            node=node,
            agate_table=agate_table,
            adapter_response=adapter_response,
            failures=failures,
        )

    def error_result(self, node, message, start_time, timing_info):
        return self._build_run_result(
            node=node,
            start_time=start_time,
            status=RunStatus.Error,
            timing_info=timing_info,
            message=message,
        )

    def ephemeral_result(self, node, start_time, timing_info):
        return self._build_run_result(
            node=node,
            start_time=start_time,
            status=RunStatus.Success,
            timing_info=timing_info,
            message=None,
        )

    def from_run_result(self, result, start_time, timing_info):
        return self._build_run_result(
            node=result.node,
            start_time=start_time,
            status=result.status,
            timing_info=timing_info,
            message=result.message,
            agate_table=result.agate_table,
            adapter_response=result.adapter_response,
            failures=result.failures,
        )

    def compile_and_execute(self, manifest, ctx):
        result = None
        with self.adapter.connection_named(
            self.node.unique_id, self.node
        ) if get_flags().INTROSPECT else nullcontext():
            ctx.node.update_event_status(node_status=RunningStatus.Compiling)
            fire_event(
                NodeCompiling(
                    node_info=ctx.node.node_info,
                )
            )
            with collect_timing_info("compile", ctx.timing.append):
                # if we fail here, we still have a compiled node to return
                # this has the benefit of showing a build path for the errant
                # model.  This calls the 'compile' method in CompileTask
                ctx.node = self.compile(manifest)

            # for ephemeral nodes, we only want to compile, not run
            if not ctx.node.is_ephemeral_model or self.run_ephemeral_models:
                ctx.node.update_event_status(node_status=RunningStatus.Executing)
                fire_event(
                    NodeExecuting(
                        node_info=ctx.node.node_info,
                    )
                )
                with collect_timing_info("execute", ctx.timing.append):
                    result = self.run(ctx.node, manifest)
                    ctx.node = result.node

        return result

    def _handle_catchable_exception(self, e, ctx):
        if e.node is None:
            e.add_node(ctx.node)

        fire_event(
            CatchableExceptionOnRun(
                exc=str(e), exc_info=traceback.format_exc(), node_info=get_node_info()
            )
        )
        return str(e)

    def _handle_internal_exception(self, e, ctx):
        fire_event(
            InternalErrorOnRun(
                build_path=self._node_build_path(), exc=str(e), node_info=get_node_info()
            )
        )
        return str(e)

    def _handle_generic_exception(self, e, ctx):
        fire_event(
            GenericExceptionOnRun(
                build_path=self._node_build_path(),
                unique_id=self.node.unique_id,
                exc=str(e),
                node_info=get_node_info(),
            )
        )
        fire_event(LogDebugStackTrace(exc_info=traceback.format_exc()))

        return str(e)

    def handle_exception(self, e, ctx):
        catchable_errors = (CompilationError, DbtRuntimeError)
        if isinstance(e, catchable_errors):
            error = self._handle_catchable_exception(e, ctx)
        elif isinstance(e, DbtInternalError):
            error = self._handle_internal_exception(e, ctx)
        else:
            error = self._handle_generic_exception(e, ctx)
        return error

    def safe_run(self, manifest):
        started = time.time()
        ctx = ExecutionContext(self.node)
        error = None
        result = None

        try:
            result = self.compile_and_execute(manifest, ctx)
        except Exception as e:
            error = self.handle_exception(e, ctx)
        finally:
            exc_str = self._safe_release_connection()

            # if releasing failed and the result doesn't have an error yet, set
            # an error
            if (
                exc_str is not None
                and result is not None
                and result.status != NodeStatus.Error
                and error is None
            ):
                error = exc_str

        if error is not None:
            result = self.error_result(ctx.node, error, started, ctx.timing)
        elif result is not None:
            result = self.from_run_result(result, started, ctx.timing)
        else:
            result = self.ephemeral_result(ctx.node, started, ctx.timing)
        return result

    def _safe_release_connection(self):
        """Try to release a connection. If an exception is hit, log and return
        the error string.
        """
        try:
            self.adapter.release_connection()
        except Exception as exc:
            fire_event(
                NodeConnectionReleaseError(
                    node_name=self.node.name, exc=str(exc), exc_info=traceback.format_exc()
                )
            )
            return str(exc)

        return None

    def before_execute(self):
        raise NotImplementedError()

    def execute(self, compiled_node, manifest):
        raise NotImplementedError()

    def run(self, compiled_node, manifest):
        return self.execute(compiled_node, manifest)

    def after_execute(self, result):
        raise NotImplementedError()

    def _skip_caused_by_ephemeral_failure(self):
        if self.skip_cause is None or self.skip_cause.node is None:
            return False
        return self.skip_cause.node.is_ephemeral_model

    def on_skip(self):
        schema_name = self.node.schema
        node_name = self.node.name

        error_message = None
        if not self.node.is_ephemeral_model:
            # if this model was skipped due to an upstream ephemeral model
            # failure, print a special 'error skip' message.
            if self._skip_caused_by_ephemeral_failure():
                fire_event(
                    LogSkipBecauseError(
                        schema=schema_name,
                        relation=node_name,
                        index=self.node_index,
                        total=self.num_nodes,
                    )
                )
                print_run_result_error(result=self.skip_cause, newline=False)
                if self.skip_cause is None:  # mypy appeasement
                    raise DbtInternalError(
                        "Skip cause not set but skip was somehow caused by an ephemeral failure"
                    )
                # set an error so dbt will exit with an error code
                error_message = (
                    "Compilation Error in {}, caused by compilation error "
                    "in referenced ephemeral model {}".format(
                        self.node.unique_id, self.skip_cause.node.unique_id
                    )
                )
            else:
                # 'skipped' nodes should not have a value for 'node_finished_at'
                # they do have 'node_started_at', which is set in GraphRunnableTask.call_runner
                self.node.update_event_status(node_status=RunStatus.Skipped)
                fire_event(
                    SkippingDetails(
                        resource_type=self.node.resource_type,
                        schema=schema_name,
                        node_name=node_name,
                        index=self.node_index,
                        total=self.num_nodes,
                        node_info=self.node.node_info,
                    )
                )

        node_result = RunResult.from_node(self.node, RunStatus.Skipped, error_message)
        return node_result

    def do_skip(self, cause=None):
        self.skip = True
        self.skip_cause = cause


def resource_types_from_args(
    args: Flags, all_resource_values: Set[NodeType], default_resource_values: Set[NodeType]
) -> Set[NodeType]:

    if not args.resource_types:
        resource_types = default_resource_values
    else:
        # This is a list of strings, not NodeTypes
        arg_resource_types = set(args.resource_types)

        if "all" in arg_resource_types:
            arg_resource_types.remove("all")
            arg_resource_types.update(all_resource_values)
        if "default" in arg_resource_types:
            arg_resource_types.remove("default")
            arg_resource_types.update(default_resource_values)
        # Convert to a set of NodeTypes now that the non-NodeType strings are gone
        resource_types = set([NodeType(rt) for rt in list(arg_resource_types)])

    if args.exclude_resource_types:
        # Convert from a list of strings to a set of NodeTypes
        exclude_resource_types = set([NodeType(rt) for rt in args.exclude_resource_types])
        resource_types = resource_types - exclude_resource_types

    return resource_types
