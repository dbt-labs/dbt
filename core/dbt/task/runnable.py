import os
import time
from abc import abstractmethod
from concurrent.futures import as_completed
from datetime import datetime
from multiprocessing.dummy import Pool as ThreadPool
from pathlib import Path
from typing import AbstractSet, Dict, Iterable, List, Optional, Set, Tuple, Type, Union

from opentelemetry import context, trace
from opentelemetry.trace import SpanContext, StatusCode

import dbt.exceptions
import dbt.tracking
import dbt.utils
import dbt_common.utils.formatting
from dbt.adapters.base import BaseAdapter, BaseRelation
from dbt.adapters.factory import get_adapter
from dbt.artifacts.schemas.results import (
    BaseResult,
    NodeStatus,
    RunningStatus,
    RunStatus,
)
from dbt.artifacts.schemas.run import RunExecutionResult, RunResult
from dbt.cli.flags import Flags
from dbt.config.runtime import RuntimeConfig
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.nodes import Exposure, ResultNode
from dbt.contracts.state import PreviousState
from dbt.events.types import (
    ArtifactWritten,
    ConcurrencyLine,
    DefaultSelector,
    EndRunResult,
    GenericExceptionOnRun,
    LogCancelLine,
    MarkSkippedChildren,
    NodeFinished,
    NodeStart,
    NothingToDo,
    QueryCancelationUnsupported,
    SkippingDetails,
)
from dbt.exceptions import DbtInternalError, DbtRuntimeError, FailFastError
from dbt.flags import get_flags
from dbt.graph import (
    GraphQueue,
    NodeSelector,
    SelectionSpec,
    UniqueId,
    parse_difference,
)
from dbt.parser.manifest import write_manifest
from dbt.task import group_lookup
from dbt.task.base import BaseRunner, ConfiguredTask
from dbt.task.printer import print_run_end_messages, print_run_result_error
from dbt_common.context import _INVOCATION_CONTEXT_VAR, get_invocation_context
from dbt_common.dataclass_schema import StrEnum
from dbt_common.events.contextvars import log_contextvars, task_contextvars
from dbt_common.events.functions import fire_event, warn_or_error
from dbt_common.events.types import Formatting
from dbt_common.exceptions import NotImplementedError

RESULT_FILE_NAME = "run_results.json"


class GraphRunnableMode(StrEnum):
    Topological = "topological"
    Independent = "independent"


def mark_node_as_skipped(
    node: ResultNode, executed_node_ids: Set[str], message: Optional[str]
) -> Optional[RunResult]:
    if node.unique_id not in executed_node_ids:
        return RunResult.from_node(node, RunStatus.Skipped, message)
    return None


class GraphRunnableTask(ConfiguredTask):
    MARK_DEPENDENT_ERRORS_STATUSES = [NodeStatus.Error, NodeStatus.PartialSuccess]

    def __init__(self, args: Flags, config: RuntimeConfig, manifest: Manifest) -> None:
        super().__init__(args, config, manifest)
        self.config = config
        self._flattened_nodes: Optional[List[ResultNode]] = None
        self._raise_next_tick: Optional[DbtRuntimeError] = None
        self._skipped_children: Dict[str, Optional[RunResult]] = {}
        self.job_queue: Optional[GraphQueue] = None
        self.node_results: List[BaseResult] = []
        self.num_nodes: int = 0
        self.previous_state: Optional[PreviousState] = None
        self.previous_defer_state: Optional[PreviousState] = None
        self.run_count: int = 0
        self.started_at: float = 0
        self._node_span_context_mapping: Dict[str, SpanContext] = {}

        if self.args.state:
            self.previous_state = PreviousState(
                state_path=self.args.state,
                target_path=Path(self.config.target_path),
                project_root=Path(self.config.project_root),
            )

        if self.args.defer_state:
            self.previous_defer_state = PreviousState(
                state_path=self.args.defer_state,
                target_path=Path(self.config.target_path),
                project_root=Path(self.config.project_root),
            )

    def index_offset(self, value: int) -> int:
        return value

    @property
    def selection_arg(self):
        return self.args.select

    @property
    def exclusion_arg(self):
        return self.args.exclude

    def get_selection_spec(self) -> SelectionSpec:
        default_selector_name = self.config.get_default_selector_name()
        spec: Union[SelectionSpec, bool]
        if hasattr(self.args, "inline") and self.args.inline:
            # We want an empty selection spec.
            spec = parse_difference(None, None)
        elif self.args.selector:
            # use pre-defined selector (--selector)
            spec = self.config.get_selector(self.args.selector)
        elif not (self.selection_arg or self.exclusion_arg) and default_selector_name:
            # use pre-defined selector (--selector) with default: true
            fire_event(DefaultSelector(name=default_selector_name))
            spec = self.config.get_selector(default_selector_name)
        else:
            # This is what's used with no default selector and no selection
            # use --select and --exclude args
            spec = parse_difference(self.selection_arg, self.exclusion_arg)
        # mypy complains because the return values of get_selector and parse_difference
        # are different
        return spec  # type: ignore

    @abstractmethod
    def get_node_selector(self) -> NodeSelector:
        raise NotImplementedError(f"get_node_selector not implemented for task {type(self)}")

    def defer_to_manifest(self):
        deferred_manifest = self._get_deferred_manifest()
        if deferred_manifest is None:
            return
        if self.manifest is None:
            raise DbtInternalError(
                "Expected to defer to manifest, but there is no runtime manifest to defer from!"
            )
        self.manifest.merge_from_artifact(other=deferred_manifest)

    def get_graph_queue(self) -> GraphQueue:
        selector = self.get_node_selector()
        # Following uses self.selection_arg and self.exclusion_arg
        spec = self.get_selection_spec()

        preserve_edges = True
        if self.get_run_mode() == GraphRunnableMode.Independent:
            preserve_edges = False

        return selector.get_graph_queue(spec, preserve_edges)

    def get_run_mode(self) -> GraphRunnableMode:
        return GraphRunnableMode.Topological

    def _runtime_initialize(self):
        self.compile_manifest()
        if self.manifest is None or self.graph is None:
            raise DbtInternalError("_runtime_initialize never loaded the graph!")

        self.job_queue = self.get_graph_queue()

        # we use this a couple of times. order does not matter.
        self._flattened_nodes = []
        for uid in self.job_queue.get_selected_nodes():
            if uid in self.manifest.nodes:
                self._flattened_nodes.append(self.manifest.nodes[uid])
            elif uid in self.manifest.sources:
                self._flattened_nodes.append(self.manifest.sources[uid])
            elif uid in self.manifest.saved_queries:
                self._flattened_nodes.append(self.manifest.saved_queries[uid])
            elif uid in self.manifest.unit_tests:
                self._flattened_nodes.append(self.manifest.unit_tests[uid])
            elif uid in self.manifest.exposures:
                self._flattened_nodes.append(self.manifest.exposures[uid])
            else:
                raise DbtInternalError(
                    f"Node selection returned {uid}, expected a node, a source, or a unit test"
                )

        self.num_nodes = len([n for n in self._flattened_nodes if not n.is_ephemeral_model])

    def raise_on_first_error(self) -> bool:
        return False

    def get_runner_type(self, node) -> Optional[Type[BaseRunner]]:
        raise NotImplementedError("Not Implemented")

    def result_path(self) -> str:
        return os.path.join(self.config.project_target_path, RESULT_FILE_NAME)

    def get_runner(self, node) -> BaseRunner:
        adapter = get_adapter(self.config)
        run_count: int = 0
        num_nodes: int = 0

        if node.is_ephemeral_model:
            run_count = 0
            num_nodes = 0
        else:
            self.run_count += 1
            run_count = self.run_count
            num_nodes = self.num_nodes

        cls = self.get_runner_type(node)

        if cls is None:
            raise DbtInternalError("Could not find runner type for node.")

        return cls(self.config, adapter, node, run_count, num_nodes)

    def call_runner(self, runner: BaseRunner) -> RunResult:
        tracer = trace.get_tracer("dbt-runner")
        node_info = runner.node.node_info
        model_span = tracer.start_span(node_info["unique_id"])
        ctx = trace.set_span_in_context(model_span)
        token = context.attach(ctx)
        self._node_span_context_mapping[node_info["unique_id"]] = model_span.get_span_context()
        for parent_node in runner.node.depends_on.nodes:
            if parent_node in self._node_span_context_mapping:
                model_span.add_link(
                    self._node_span_context_mapping[parent_node], {"model_name": parent_node}
                )
        with log_contextvars(node_info=node_info):
            runner.node.update_event_status(
                started_at=datetime.utcnow().isoformat(), node_status=RunningStatus.Started
            )
            fire_event(
                NodeStart(
                    node_info=node_info,
                )
            )
            try:
                result = runner.run_with_hooks(self.manifest)
            except (KeyboardInterrupt, SystemExit) as exe:
                result = None
                thread_exception: Union[KeyboardInterrupt, SystemExit, Exception] = exe
                raise
            except Exception as e:
                result = None
                thread_exception = e
            finally:
                context.detach(token)
                model_span.end()
                if result is not None:
                    fire_event(
                        NodeFinished(
                            node_info=node_info,
                            run_result=result.to_msg_dict(),
                        )
                    )
                else:
                    msg = f"Exception on worker thread. {thread_exception}"

                    fire_event(
                        GenericExceptionOnRun(
                            unique_id=runner.node.unique_id,
                            exc=str(thread_exception),
                            node_info=node_info,
                        )
                    )

                    result = RunResult(
                        status=RunStatus.Error,  # type: ignore
                        timing=[],
                        thread_id="",
                        execution_time=0.0,
                        adapter_response={},
                        message=msg,
                        failures=None,
                        batch_results=None,
                        node=runner.node,
                    )

            # `_event_status` dict is only used for logging.  Make sure
            # it gets deleted when we're done with it
            runner.node.clear_event_status()

        fail_fast = get_flags().FAIL_FAST

        if result.status in (NodeStatus.Error, NodeStatus.Fail, NodeStatus.PartialSuccess):
            model_span.set_status(StatusCode.ERROR)

        if (
            result.status in (NodeStatus.Error, NodeStatus.Fail, NodeStatus.PartialSuccess)
            and fail_fast
        ):
            self._raise_next_tick = FailFastError(
                msg="Failing early due to test failure or runtime error",
                result=result,
                node=getattr(result, "node", None),
            )
        elif result.status == NodeStatus.Error and self.raise_on_first_error():
            # if we raise inside a thread, it'll just get silently swallowed.
            # stash the error message we want here, and it will check the
            # next 'tick' - should be soon since our thread is about to finish!
            self._raise_next_tick = DbtRuntimeError(result.message)

        return result

    def _submit(self, pool, args, callback):
        """If the caller has passed the magic 'single-threaded' flag, call the
        function directly instead of pool.apply_async. The single-threaded flag
         is intended for gathering more useful performance information about
        what happens beneath `call_runner`, since python's default profiling
        tools ignore child threads.

        This does still go through the callback path for result collection.
        """
        if self.config.args.single_threaded:
            callback(self.call_runner(*args))
        else:
            pool.apply_async(self.call_runner, args=args, callback=callback)

    def _raise_set_error(self):
        if self._raise_next_tick is not None:
            raise self._raise_next_tick

    def run_queue(self, pool):
        """Given a pool, submit jobs from the queue to the pool."""
        if self.job_queue is None:
            raise DbtInternalError("Got to run_queue with no job queue set")

        def callback(result):
            """Note: mark_done, at a minimum, must happen here or dbt will
            deadlock during ephemeral result error handling!
            """
            self._handle_result(result)

            if self.job_queue is None:
                raise DbtInternalError("Got to run_queue callback with no job queue set")
            self.job_queue.mark_done(result.node.unique_id)

        while not self.job_queue.empty():
            self.handle_job_queue(pool, callback)

        # block on completion
        if get_flags().FAIL_FAST:
            # checkout for an errors after task completion in case of
            # fast failure
            while self.job_queue.wait_until_something_was_done():
                self._raise_set_error()
        else:
            # wait until every task will be complete
            self.job_queue.join()

        # if an error got set during join(), raise it.
        self._raise_set_error()

        return

    # The build command overrides this
    def handle_job_queue(self, pool, callback):
        node = self.job_queue.get()
        self._raise_set_error()
        runner = self.get_runner(node)
        # we finally know what we're running! Make sure we haven't decided
        # to skip it due to upstream failures
        if runner.node.unique_id in self._skipped_children:
            cause = self._skipped_children.pop(runner.node.unique_id)
            runner.do_skip(cause=cause)
        args = [runner]
        self._submit(pool, args, callback)

    def _handle_result(self, result: RunResult) -> None:
        """Mark the result as completed, insert the `CompileResultNode` into
        the manifest, and mark any descendants (potentially with a 'cause' if
        the result was an ephemeral model) as skipped.
        """
        is_ephemeral = result.node.is_ephemeral_model
        if not is_ephemeral:
            self.node_results.append(result)

        node = result.node

        if self.manifest is None:
            raise DbtInternalError("manifest was None in _handle_result")

        # If result.status == NodeStatus.Error, plus Fail for build command
        if result.status in self.MARK_DEPENDENT_ERRORS_STATUSES:
            if is_ephemeral:
                cause = result
            else:
                cause = None
            self._mark_dependent_errors(node.unique_id, result, cause)

    def _cancel_connections(self, pool):
        """Given a pool, cancel all adapter connections and wait until all
        runners gentle terminates.
        """
        pool.close()
        pool.terminate()

        adapter = get_adapter(self.config)

        if not adapter.is_cancelable():
            fire_event(QueryCancelationUnsupported(type=adapter.type()))
        else:
            with adapter.connection_named("master"):
                for conn_name in adapter.cancel_open_connections():
                    if self.manifest is not None:
                        node = self.manifest.nodes.get(conn_name)
                        if node is not None and node.is_ephemeral_model:
                            continue
                    # if we don't have a manifest/don't have a node, print
                    # anyway.
                    fire_event(LogCancelLine(conn_name=conn_name))

        pool.join()

    def execute_nodes(self):
        num_threads = self.config.threads

        pool = ThreadPool(num_threads, self._pool_thread_initializer, [get_invocation_context()])
        try:
            self.run_queue(pool)
        except FailFastError as failure:
            self._cancel_connections(pool)

            executed_node_ids = {r.node.unique_id for r in self.node_results}
            message = "Skipping due to fail_fast"

            for node in self._flattened_nodes:
                if node.unique_id not in executed_node_ids:
                    self.node_results.append(
                        mark_node_as_skipped(node, executed_node_ids, message)
                    )

            print_run_result_error(failure.result)
            # ensure information about all nodes is propagated to run results when failing fast
            return self.node_results
        except (KeyboardInterrupt, SystemExit):
            run_result = self.get_result(
                results=self.node_results,
                elapsed_time=time.time() - self.started_at,
                generated_at=datetime.utcnow(),
            )

            if self.args.write_json and hasattr(run_result, "write"):
                run_result.write(self.result_path())
                fire_event(
                    ArtifactWritten(
                        artifact_type=run_result.__class__.__name__,
                        artifact_path=self.result_path(),
                    )
                )

            self._cancel_connections(pool)
            print_run_end_messages(self.node_results, keyboard_interrupt=True)

            raise

        pool.close()
        pool.join()

        return self.node_results

    @staticmethod
    def _pool_thread_initializer(invocation_context):
        _INVOCATION_CONTEXT_VAR.set(invocation_context)

    def _mark_dependent_errors(
        self, node_id: str, result: RunResult, cause: Optional[RunResult]
    ) -> None:
        if self.graph is None:
            raise DbtInternalError("graph is None in _mark_dependent_errors")
        fire_event(
            MarkSkippedChildren(
                unique_id=node_id,
                status=result.status,
                run_result=result.to_msg_dict(),
            )
        )
        for dep_node_id in self.graph.get_dependent_nodes(UniqueId(node_id)):
            self._skipped_children[dep_node_id] = cause

    def populate_adapter_cache(
        self, adapter, required_schemas: Optional[Set[BaseRelation]] = None
    ):
        if not self.args.populate_cache:
            return

        if self.manifest is None:
            raise DbtInternalError("manifest was None in populate_adapter_cache")

        start_populate_cache = time.perf_counter()
        # the cache only cares about executable nodes
        cachable_nodes = [
            node
            for node in self.manifest.nodes.values()
            if (node.is_relational and not node.is_ephemeral_model and not node.is_external_node)
        ]

        if get_flags().CACHE_SELECTED_ONLY is True:
            adapter.set_relations_cache(cachable_nodes, required_schemas=required_schemas)
        else:
            adapter.set_relations_cache(cachable_nodes)
        cache_populate_time = time.perf_counter() - start_populate_cache
        if dbt.tracking.active_user is not None:
            dbt.tracking.track_runnable_timing(
                {"adapter_cache_construction_elapsed": cache_populate_time}
            )

    def before_run(self, adapter: BaseAdapter, selected_uids: AbstractSet[str]) -> RunStatus:
        with adapter.connection_named("master"):
            self.defer_to_manifest()
            self.populate_adapter_cache(adapter)
            return RunStatus.Success

    def after_run(self, adapter, results) -> None:
        pass

    def print_results_line(self, node_results, elapsed):
        pass

    def execute_with_hooks(self, selected_uids: AbstractSet[str]):
        adapter = get_adapter(self.config)

        fire_event(Formatting(""))
        fire_event(
            ConcurrencyLine(
                num_threads=self.config.threads,
                target_name=self.config.target_name,
                node_count=self.num_nodes,
            )
        )
        fire_event(Formatting(""))

        self.started_at = time.time()
        try:
            before_run_status = self.before_run(adapter, selected_uids)
            if before_run_status == RunStatus.Success or (
                not get_flags().skip_nodes_if_on_run_start_fails
            ):
                res = self.execute_nodes()
            else:
                executed_node_ids = {
                    r.node.unique_id for r in self.node_results if hasattr(r, "node")
                }

                res = []

                for index, node in enumerate(self._flattened_nodes or []):
                    group = group_lookup.get(node.unique_id)

                    if node.unique_id not in executed_node_ids:
                        fire_event(
                            SkippingDetails(
                                resource_type=node.resource_type,
                                schema=node.schema,
                                node_name=node.name,
                                index=index + 1,
                                total=self.num_nodes,
                                node_info=node.node_info,
                                group=group,
                            )
                        )
                        skipped_node_result = mark_node_as_skipped(node, executed_node_ids, None)
                        if skipped_node_result:
                            self.node_results.append(skipped_node_result)

            self.after_run(adapter, res)
        finally:
            adapter.cleanup_connections()
            elapsed = time.time() - self.started_at
            self.print_results_line(self.node_results, elapsed)
            result = self.get_result(
                results=self.node_results, elapsed_time=elapsed, generated_at=datetime.utcnow()
            )

        return result

    def run(self):
        """
        Run dbt for the query, based on the graph.
        """
        # We set up a context manager here with "task_contextvars" because we
        # need the project_root in runtime_initialize.
        with task_contextvars(project_root=self.config.project_root):
            self._runtime_initialize()

            if self._flattened_nodes is None:
                raise DbtInternalError(
                    "after _runtime_initialize, _flattened_nodes was still None"
                )

            if len(self._flattened_nodes) == 0:
                warn_or_error(NothingToDo())
                result = self.get_result(
                    results=[],
                    generated_at=datetime.utcnow(),
                    elapsed_time=0.0,
                )
            else:
                selected_uids = frozenset(n.unique_id for n in self._flattened_nodes)
                result = self.execute_with_hooks(selected_uids)

        # We have other result types here too, including FreshnessResult
        if isinstance(result, RunExecutionResult):
            result_msgs = [result.to_msg_dict() for result in result.results]
            fire_event(
                EndRunResult(
                    results=result_msgs,
                    generated_at=result.generated_at.strftime("%Y-%m-%dT%H:%M:%SZ"),
                    elapsed_time=result.elapsed_time,
                    success=GraphRunnableTask.interpret_results(result.results),
                )
            )

        if self.args.write_json:
            write_manifest(self.manifest, self.config.project_target_path)
            if hasattr(result, "write"):
                result.write(self.result_path())
                fire_event(
                    ArtifactWritten(
                        artifact_type=result.__class__.__name__, artifact_path=self.result_path()
                    )
                )

        self.task_end_messages(result.results)
        return result

    @classmethod
    def interpret_results(cls, results):
        if results is None:
            return False

        num_runtime_errors = len([r for r in results if r.status == NodeStatus.RuntimeErr])
        num_errors = len([r for r in results if r.status == NodeStatus.Error])
        num_fails = len([r for r in results if r.status == NodeStatus.Fail])
        num_skipped = len(
            [
                r
                for r in results
                if r.status == NodeStatus.Skipped and not isinstance(r.node, Exposure)
            ]
        )
        num_partial_success = len([r for r in results if r.status == NodeStatus.PartialSuccess])
        num_total = num_runtime_errors + num_errors + num_fails + num_skipped + num_partial_success
        return num_total == 0

    def get_model_schemas(self, adapter, selected_uids: Iterable[str]) -> Set[BaseRelation]:
        if self.manifest is None:
            raise DbtInternalError("manifest was None in get_model_schemas")
        result: Set[BaseRelation] = set()

        for node in self.manifest.nodes.values():
            if node.unique_id not in selected_uids:
                continue
            if node.is_relational and not node.is_ephemeral:
                relation = adapter.Relation.create_from(self.config, node)
                result.add(relation.without_identifier())

        return result

    def create_schemas(self, adapter, required_schemas: Set[BaseRelation]):
        # we want the string form of the information schema database
        required_databases: Set[BaseRelation] = set()
        for required in required_schemas:
            db_only = required.include(database=True, schema=False, identifier=False)
            required_databases.add(db_only)

        existing_schemas_lowered: Set[Tuple[Optional[str], Optional[str]]]
        existing_schemas_lowered = set()

        def list_schemas(db_only: BaseRelation) -> List[Tuple[Optional[str], str]]:
            # the database can be None on some warehouses that don't support it
            database_quoted: Optional[str]
            db_lowercase = dbt_common.utils.formatting.lowercase(db_only.database)
            if db_only.database is None:
                database_quoted = None
            else:
                database_quoted = str(db_only)

            # we should never create a null schema, so just filter them out
            return [
                (db_lowercase, s.lower())
                for s in adapter.list_schemas(database_quoted)
                if s is not None
            ]

        def create_schema(relation: BaseRelation) -> None:
            db = relation.database or ""
            schema = relation.schema
            with adapter.connection_named(f"create_{db}_{schema}"):
                adapter.create_schema(relation)

        list_futures = []
        create_futures = []

        # TODO: following has a mypy issue because profile and project config
        # defines threads as int and HasThreadingConfig defines it as Optional[int]
        with dbt_common.utils.executor(self.config) as tpe:  # type: ignore
            for req in required_databases:
                if req.database is None:
                    name = "list_schemas"
                else:
                    name = f"list_{req.database}"
                fut = tpe.submit_connected(adapter, name, list_schemas, req)
                list_futures.append(fut)

            for ls_future in as_completed(list_futures):
                existing_schemas_lowered.update(ls_future.result())

            for info in required_schemas:
                if info.schema is None:
                    # we are not in the business of creating null schemas, so
                    # skip this
                    continue
                db: Optional[str] = info.database
                db_lower: Optional[str] = dbt_common.utils.formatting.lowercase(db)
                schema: str = info.schema

                db_schema = (db_lower, schema.lower())
                if db_schema not in existing_schemas_lowered:
                    existing_schemas_lowered.add(db_schema)
                    fut = tpe.submit_connected(
                        adapter, f'create_{info.database or ""}_{info.schema}', create_schema, info
                    )
                    create_futures.append(fut)

            for create_future in as_completed(create_futures):
                # trigger/re-raise any exceptions while creating schemas
                create_future.result()

    def get_result(self, results, elapsed_time, generated_at):
        return RunExecutionResult(
            results=results,
            elapsed_time=elapsed_time,
            generated_at=generated_at,
            args=dbt.utils.args_to_dict(self.args),
        )

    def task_end_messages(self, results) -> None:
        print_run_end_messages(results)

    def _get_previous_state(self) -> Optional[Manifest]:
        state = self.previous_defer_state or self.previous_state
        if not state:
            raise DbtRuntimeError(
                "--state or --defer-state are required for deferral, but neither was provided"
            )

        if not state.manifest:
            raise DbtRuntimeError(f'Could not find manifest in --state path: "{state.state_path}"')
        return state.manifest

    def _get_deferred_manifest(self) -> Optional[Manifest]:
        return self._get_previous_state() if self.args.defer else None
