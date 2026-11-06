from __future__ import annotations

import functools
import threading
import time
from copy import deepcopy
from dataclasses import asdict
from dataclasses import replace as dataclass_replace
from datetime import datetime, timezone
from typing import (
    AbstractSet,
    Any,
    Callable,
    Dict,
    Iterable,
    List,
    Optional,
    Set,
    Tuple,
    Type,
    cast,
)

from opentelemetry.trace import StatusCode

from dbt import tracking, utils
from dbt.adapters.base import BaseAdapter, BaseRelation
from dbt.adapters.capability import Capability
from dbt.adapters.catalogs import DbtCatalogIntegrationNotFoundError
from dbt.adapters.events.types import FinishedRunningStats
from dbt.adapters.exceptions import MissingMaterializationError
from dbt.artifacts.resources import Catalog, Hook
from dbt.artifacts.schemas.batch_results import BatchResults, BatchType
from dbt.artifacts.schemas.overload_results import OverloadResults
from dbt.artifacts.schemas.results import (
    NodeStatus,
    RunningStatus,
    RunStatus,
    TimingInfo,
    collect_timing_info,
)
from dbt.artifacts.schemas.run import RunResult
from dbt.cli.flags import Flags
from dbt.clients.jinja import MacroGenerator
from dbt.config import RuntimeConfig
from dbt.context.providers import generate_runtime_model_context
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.nodes import (
    BatchContext,
    FunctionNode,
    HookNode,
    ModelConfig,
    ModelNode,
    ResultNode,
)
from dbt.events.types import (
    GenericExceptionOnRun,
    LogBatchResult,
    LogHookEndLine,
    LogHookStartLine,
    LogModelResult,
    LogStartBatch,
    LogStartLine,
    MicrobatchExecutionDebug,
)
from dbt.exceptions import CompilationError, DbtInternalError, DbtRuntimeError
from dbt.flags import get_flags
from dbt.graph import ResourceTypeSelector
from dbt.graph.thread_pool import DbtThreadPool
from dbt.hooks import get_hook_dict
from dbt.materializations.incremental.microbatch import MicrobatchBuilder
from dbt.node_types import NodeType, RunHookType
from dbt.task import group_lookup
from dbt.task.base import BaseRunner
from dbt.task.compile import CompileRunner, CompileTask
from dbt.task.printer import get_counts, print_run_end_messages
from dbt.utils.artifact_upload import add_artifact_produced
from dbt_common.clients.jinja import MacroProtocol
from dbt_common.dataclass_schema import dbtClassMixin
from dbt_common.events.base_types import EventLevel
from dbt_common.events.contextvars import log_contextvars
from dbt_common.events.functions import fire_event, get_invocation_id
from dbt_common.events.types import Formatting
from dbt_common.exceptions import DbtValidationError
from dbt_common.invocation import get_invocation_started_at


@functools.total_ordering
class BiggestName(str):
    def __lt__(self, other):
        return True

    def __eq__(self, other):
        return isinstance(other, self.__class__)


def _hook_list() -> List[HookNode]:
    return []


def get_hooks_by_tags(
    nodes: Iterable[ResultNode],
    match_tags: Set[str],
) -> List[HookNode]:
    matched_nodes = []
    for node in nodes:
        if not isinstance(node, HookNode):
            continue
        node_tags = node.tags
        if len(set(node_tags) & match_tags):
            matched_nodes.append(node)
    return matched_nodes


def get_hook(source, index):
    hook_dict = get_hook_dict(source)
    hook_dict.setdefault("index", index)
    Hook.validate(hook_dict)
    return Hook.from_dict(hook_dict)


def get_execution_status(sql: str, adapter: BaseAdapter) -> Tuple[RunStatus, str]:
    if not sql.strip():
        return RunStatus.Success, "OK"

    try:
        response, _ = adapter.execute(sql, auto_begin=False, fetch=False)
        status = RunStatus.Success
        message = response._message
    except (KeyboardInterrupt, SystemExit):
        raise
    except DbtRuntimeError as exc:
        status = RunStatus.Error
        message = exc.msg
    except Exception as exc:
        status = RunStatus.Error
        message = str(exc)

    return (status, message)


def _get_adapter_info(adapter, run_model_result) -> Dict[str, Any]:
    """Each adapter returns a dataclass with a flexible dictionary for
    adapter-specific fields. Only the non-'model_adapter_details' fields
    are guaranteed cross adapter."""
    return asdict(adapter.get_adapter_run_info(run_model_result.node.config)) if adapter else {}


def _get_catalog_type(model_node_config: ModelConfig, adapter: BaseAdapter) -> Optional[str]:
    catalog_name = model_node_config._extra.get("catalog_name")
    if catalog_name is None:
        return None

    try:
        return adapter.get_catalog_integration(catalog_name).catalog_type
    except DbtCatalogIntegrationNotFoundError:
        return None


def track_model_run(index, num_nodes, run_model_result, adapter=None):
    if tracking.active_user is None:
        raise DbtInternalError("cannot track model run with no active user")
    invocation_id = get_invocation_id()
    node = run_model_result.node
    has_group = True if hasattr(node, "group") and node.group else False
    if node.resource_type == NodeType.Model:
        access = node.access.value if node.access is not None else None
        contract_enforced = node.contract.enforced
        versioned = True if node.version else False
        incremental_strategy = node.config.incremental_strategy
        model_config = node.config
    else:
        access = None
        contract_enforced = False
        versioned = False
        incremental_strategy = None
        model_config = None

    tracking.track_model_run(
        {
            "invocation_id": invocation_id,
            "index": index,
            "total": num_nodes,
            "execution_time": run_model_result.execution_time,
            "run_status": str(run_model_result.status).upper(),
            "run_skipped": run_model_result.status == NodeStatus.Skipped,
            "run_error": run_model_result.status == NodeStatus.Error,
            "model_materialization": node.get_materialization(),
            "model_incremental_strategy": incremental_strategy,
            "model_id": utils.get_hash(node),
            "hashed_contents": utils.get_hashed_contents(node),
            "timing": [t.to_dict(omit_none=True) for t in run_model_result.timing],
            "language": str(node.language),
            "has_group": has_group,
            "contract_enforced": contract_enforced,
            "access": access,
            "versioned": versioned,
            "adapter_info": _get_adapter_info(adapter, run_model_result),
            "catalog_type": model_config and _get_catalog_type(model_config, adapter),
        }
    )


# make sure that we got an ok result back from a materialization
def _validate_materialization_relations_dict(inp: Dict[Any, Any], model) -> List[BaseRelation]:
    try:
        relations_value = inp["relations"]
    except KeyError:
        msg = (
            'Invalid return value from materialization, "relations" '
            "not found, got keys: {}".format(list(inp))
        )
        raise CompilationError(msg, node=model) from None

    if not isinstance(relations_value, list):
        msg = (
            'Invalid return value from materialization, "relations" '
            "not a list, got: {}".format(relations_value)
        )
        raise CompilationError(msg, node=model) from None

    relations: List[BaseRelation] = []
    for relation in relations_value:
        if not isinstance(relation, BaseRelation):
            msg = (
                "Invalid return value from materialization, "
                '"relations" contains non-Relation: {}'.format(relation)
            )
            raise CompilationError(msg, node=model)

        assert isinstance(relation, BaseRelation)
        relations.append(relation)
    return relations


class ModelRunner(CompileRunner[ModelNode]):
    def _relation_identifier(self, relation: BaseRelation) -> str:
        identifier = getattr(relation, "identifier", None)
        if identifier is not None:
            return identifier
        return relation.name or ""

    def _pointer_collides_with_source(
        self, source_relation: BaseRelation, pointer_identifier: str
    ) -> bool:
        """Whether the pointer identifier resolves to the same relation as the latest
        version's identifier.

        When the identifier component is unquoted, the warehouse resolves identifiers
        case-insensitively (e.g. Snowflake folds ``DIM_CUSTOMERS`` and ``dim_customers``
        to the same object), so compare case-insensitively. When quoted, identifiers are
        case-sensitive, so compare exactly.
        """
        source_identifier = self._relation_identifier(source_relation)
        quote_policy = getattr(source_relation, "quote_policy", None)
        identifier_quoted = bool(getattr(quote_policy, "identifier", True))
        if identifier_quoted:
            return source_identifier == pointer_identifier
        return source_identifier.lower() == pointer_identifier.lower()

    def _latest_version_pointer_identifier(
        self,
        model: ModelNode,
        manifest: Manifest,
        context: Dict[str, Any],
    ) -> str:
        lvv = getattr(model.config, "latest_version_pointer", None)
        custom_alias_name = str(lvv.alias).strip() if (lvv and lvv.alias) else None
        alias_macro = manifest.find_macro_by_name(
            "generate_latest_version_pointer_alias", self.config.project_name, None
        )
        if alias_macro is not None:
            return MacroGenerator(alias_macro, context, stack=context["context_macro_stack"])(
                custom_alias_name, context["model"]
            ).strip()
        return custom_alias_name or model.name

    def _should_create_latest_version_pointer(self, model: ModelNode) -> bool:
        lvv = getattr(model.config, "latest_version_pointer", None)
        if lvv is not None and lvv.enabled is not None:
            # User explicitly set enabled — respect it regardless of flag
            enabled = lvv.enabled
        else:
            # Not explicitly set — defer to project flag (default: False)
            enabled = bool(
                getattr(get_flags(), "latest_version_pointer_enabled_by_default", False)
            )
        return (
            isinstance(model, ModelNode)
            and model.version is not None
            and model.is_latest_version
            and enabled
        )

    def _materialize_latest_version_pointer(
        self,
        manifest: Manifest,
        model: ModelNode,
        context: Dict[str, Any],
        relations: List[BaseRelation],
    ) -> List[BaseRelation]:
        if not relations or not self._should_create_latest_version_pointer(model):
            return []

        source_relation = relations[0]
        pointer_identifier = self._latest_version_pointer_identifier(model, manifest, context)

        if self._pointer_collides_with_source(source_relation, pointer_identifier):
            raise DbtRuntimeError(
                f"Cannot create latest version pointer: the latest version of '{model.name}' "
                f"is already aliased to '{pointer_identifier}'. "
                f"Set `latest_version_pointer: enabled: false` or remove the conflicting alias."
            )

        # Build a runtime-only synthetic node for the pointer view.
        # Never added to the manifest — only used to generate context for the
        # view materialization macro so it runs with the right target relation,
        # SQL, and config (grants inherited, hooks and persist_docs cleared).
        pointer_node = dataclass_replace(
            model,
            alias=pointer_identifier,
            raw_code=f"select * from {source_relation}",
            compiled_code=f"select * from {source_relation}",
            compiled=True,
            unique_id=f"{model.unique_id}__latest_version_pointer",
            config=dataclass_replace(
                model.config,
                pre_hook=[],
                post_hook=[],
                persist_docs={},
            ),
        )

        pointer_context = generate_runtime_model_context(pointer_node, self.config, manifest)

        view_materialization = manifest.find_materialization_macro_by_name(
            self.config.project_name, "view", self.adapter.type()
        )
        if view_materialization is None:
            raise DbtInternalError("Missing view materialization for latest version pointer")

        result = MacroGenerator(
            view_materialization,
            pointer_context,
            stack=pointer_context["context_macro_stack"],
        )()

        return self._materialization_relations(result, pointer_node)

    def describe_node(self) -> str:
        # TODO CL 'language' will be moved to node level when we change representation
        return f"{self.node.language} {self.node.get_materialization()} model {self.get_node_representation()}"

    def print_start_line(self):
        fire_event(
            LogStartLine(
                description=self.describe_node(),
                index=self.node_index,
                total=self.num_nodes,
                node_info=self.node.node_info,
            )
        )

    def print_result_line(self, result):
        description = self.describe_node()
        group = group_lookup.get(self.node.unique_id)
        if result.status == NodeStatus.Error:
            status = result.status
            level = EventLevel.ERROR
        else:
            status = result.message
            level = EventLevel.INFO
        fire_event(
            LogModelResult(
                description=description,
                status=status,
                index=self.node_index,
                total=self.num_nodes,
                execution_time=result.execution_time,
                node_info=self.node.node_info,
                group=group,
            ),
            level=level,
        )

    def before_execute(self) -> None:
        self.print_start_line()

    def after_execute(self, result: RunResult) -> None:
        track_model_run(self.node_index, self.num_nodes, result, adapter=self.adapter)
        self.print_result_line(result)

    def _build_run_model_result(self, model, context, elapsed_time: float = 0.0):
        result = context["load_result"]("main")
        if not result:
            raise DbtRuntimeError("main is not being called during running model")
        adapter_response = {}
        if isinstance(result.response, dbtClassMixin):
            adapter_response = result.response.to_dict(omit_none=True)
        return RunResult(
            node=model,
            status=RunStatus.Success,
            timing=[],
            thread_id=threading.current_thread().name,
            execution_time=elapsed_time,
            message=str(result.response),
            adapter_response=adapter_response,
            failures=result.get("failures"),
            batch_results=None,
        )

    def _materialization_relations(self, result: Any, model) -> List[BaseRelation]:
        if isinstance(result, str):
            msg = (
                'The materialization ("{}") did not explicitly return a '
                "list of relations to add to the cache.".format(str(model.get_materialization()))
            )
            raise CompilationError(msg, node=model)

        if isinstance(result, dict):
            return _validate_materialization_relations_dict(result, model)

        msg = (
            "Invalid return value from materialization, expected a dict "
            'with key "relations", got: {}'.format(str(result))
        )
        raise CompilationError(msg, node=model)

    def _execute_model(
        self,
        hook_ctx: Any,
        context_config: Any,
        model: ModelNode,
        context: Dict[str, Any],
        materialization_macro: MacroProtocol,
        manifest: Manifest,
    ) -> RunResult:
        relations: List[BaseRelation] = []
        try:
            result = MacroGenerator(
                materialization_macro, context, stack=context["context_macro_stack"]
            )()
            relations = self._materialization_relations(result, model)
            relations.extend(
                self._materialize_latest_version_pointer(manifest, model, context, relations)
            )
        finally:
            self.adapter.post_model_hook(context_config, hook_ctx)

        for relation in relations:
            self.adapter.cache_added(relation.incorporate(dbt_created=True))

        return self._build_run_model_result(model, context)

    def _get_materialization_macro(self, model: ModelNode, manifest: Manifest) -> MacroProtocol:
        materialization_macro = manifest.find_materialization_macro_by_name(
            self.config.project_name, model.get_materialization(), self.adapter.type()
        )
        if materialization_macro is None:
            raise MissingMaterializationError(
                materialization=model.get_materialization(), adapter_type=self.adapter.type()
            )
        supported_languages = getattr(materialization_macro, "supported_languages", None)
        if supported_languages is not None:
            if model.language not in supported_languages:
                str_langs = [str(lang) for lang in supported_languages]
                raise DbtValidationError(
                    f'Materialization "{materialization_macro.name}" only supports languages {str_langs}; '
                    f'got "{model.language}"'
                )
        return materialization_macro

    def execute(self, model, manifest) -> RunResult:
        context = generate_runtime_model_context(model, self.config, manifest)

        if "config" not in context:
            raise DbtInternalError(
                "Invalid materialization context generated, missing config: {}".format(context)
            )

        materialization_macro = self._get_materialization_macro(model, manifest)
        hook_ctx = self.adapter.pre_model_hook(context["config"])

        return self._execute_model(
            hook_ctx, context["config"], model, context, materialization_macro, manifest
        )


class MicrobatchBatchRunner(ModelRunner):
    """Handles the running of individual batches"""

    def __init__(
        self,
        config,
        adapter,
        node,
        node_index: int,
        num_nodes: int,
        batch_idx: int,
        batches: Dict[int, BatchType],
        relation_exists: bool,
        incremental_batch: bool,
    ):
        super().__init__(config, adapter, node, node_index, num_nodes)

        self.batch_idx = batch_idx
        self.batches = batches
        self.relation_exists = relation_exists
        self.incremental_batch = incremental_batch

    def describe_batch(self) -> str:
        batch_start = self.batches[self.batch_idx][0]
        formatted_batch_start = MicrobatchBuilder.format_batch_start(
            batch_start, self.node.config.batch_size
        )
        return f"batch {formatted_batch_start} of {self.get_node_representation()}"

    def print_result_line(self, result: RunResult):
        if result.status == NodeStatus.Error:
            status = result.status
            level = EventLevel.ERROR
        elif result.status == NodeStatus.Skipped:
            status = result.status
            level = EventLevel.INFO
        else:
            status = result.message
            level = EventLevel.INFO

        fire_event(
            LogBatchResult(
                description=self.describe_batch(),
                status=status,
                batch_index=self.batch_idx + 1,
                total_batches=len(self.batches),
                execution_time=result.execution_time,
                node_info=self.node.node_info,
                group=group_lookup.get(self.node.unique_id),
            ),
            level=level,
        )

    def print_start_line(self) -> None:
        fire_event(
            LogStartBatch(
                description=self.describe_batch(),
                batch_index=self.batch_idx + 1,
                total_batches=len(self.batches),
                node_info=self.node.node_info,
            )
        )

    def should_run_in_parallel(self) -> bool:
        if not self.adapter.supports(Capability.MicrobatchConcurrency):
            run_in_parallel = False
        elif not self.relation_exists:
            # If the relation doesn't exist, we can't run in parallel
            run_in_parallel = False
        elif self.node.config.concurrent_batches is not None:
            # If the relation exists and the `concurrent_batches` config isn't None, use the config value
            run_in_parallel = self.node.config.concurrent_batches
        else:
            # If the relation exists, the `concurrent_batches` config is None, check if the model self references `this`.
            # If the model self references `this` then we assume the model batches _can't_ be run in parallel
            run_in_parallel = not self.node.has_this

        return run_in_parallel

    def on_skip(self) -> RunResult:
        result = RunResult(
            node=self.node,
            status=RunStatus.Skipped,
            timing=[],
            thread_id=threading.current_thread().name,
            execution_time=0.0,
            message="SKIPPED",
            adapter_response={},
            failures=1,
            batch_results=BatchResults(failed=[self.batches[self.batch_idx]]),
        )
        self.print_result_line(result=result)
        return result

    def error_result(self, node, message, start_time, timing_info) -> RunResult:
        """Necessary to return a result with a batch result

        Called by `BaseRunner.safe_run` when an error occurs
        """
        return self._build_run_result(
            node=node,
            start_time=start_time,
            status=RunStatus.Error,
            timing_info=timing_info,
            message=message,
            batch_results=BatchResults(failed=[self.batches[self.batch_idx]]),
        )

    def compile(self, manifest: Manifest):
        batch = self.batches[self.batch_idx]

        # LEGACY: Set start/end in context prior to re-compiling (Will be removed for 1.10+)
        # TODO: REMOVE before 1.10 GA
        self.node.config["__dbt_internal_microbatch_event_time_start"] = batch[0]
        self.node.config["__dbt_internal_microbatch_event_time_end"] = batch[1]
        # Create batch context on model node prior to re-compiling
        self.node.batch = BatchContext(
            id=MicrobatchBuilder.batch_id(batch[0], self.node.config.batch_size),
            event_time_start=batch[0],
            event_time_end=batch[1],
        )
        # Recompile node to re-resolve refs with event time filters rendered, update context
        self.compiler.compile_node(
            self.node,
            manifest,
            {},
            split_suffix=MicrobatchBuilder.format_batch_start(
                batch[0], self.node.config.batch_size
            ),
        )

        return self.node

    def _build_succesful_run_batch_result(
        self,
        model: ModelNode,
        context: Dict[str, Any],
        batch: BatchType,
        elapsed_time: float = 0.0,
    ) -> RunResult:
        run_result = self._build_run_model_result(model, context, elapsed_time)
        run_result.batch_results = BatchResults(successful=[batch])
        return run_result

    def _build_failed_run_batch_result(
        self,
        model: ModelNode,
        batch: BatchType,
        elapsed_time: float = 0.0,
    ) -> RunResult:
        return RunResult(
            node=model,
            status=RunStatus.Error,
            timing=[],
            thread_id=threading.current_thread().name,
            execution_time=elapsed_time,
            message="ERROR",
            adapter_response={},
            failures=1,
            batch_results=BatchResults(failed=[batch]),
        )

    def _execute_microbatch_materialization(
        self,
        model: ModelNode,
        context: Dict[str, Any],
        materialization_macro: MacroProtocol,
    ) -> RunResult:

        batch = self.batches[self.batch_idx]
        # call materialization_macro to get a batch-level run result
        start_time = time.perf_counter()
        try:
            # Update jinja context with batch context members
            jinja_context = MicrobatchBuilder.build_jinja_context_for_batch(
                model=model,
                incremental_batch=self.incremental_batch,
            )
            context.update(jinja_context)

            # Materialize batch and cache any materialized relations
            result = MacroGenerator(
                materialization_macro, context, stack=context["context_macro_stack"]
            )()
            for relation in self._materialization_relations(result, model):
                self.adapter.cache_added(relation.incorporate(dbt_created=True))

            # Build result of executed batch
            batch_run_result = self._build_succesful_run_batch_result(
                model, context, batch, time.perf_counter() - start_time
            )
            batch_result = batch_run_result

            # At least one batch has been inserted successfully!
            # Can proceed incrementally + in parallel
            self.relation_exists = True

        except (KeyboardInterrupt, SystemExit):
            # reraise it for GraphRunnableTask.execute_nodes to handle
            raise
        except Exception as e:
            fire_event(
                GenericExceptionOnRun(
                    unique_id=self.node.unique_id,
                    exc=f"Exception on worker thread. {str(e)}",
                    node_info=self.node.node_info,
                )
            )
            batch_run_result = self._build_failed_run_batch_result(
                model, batch, time.perf_counter() - start_time
            )

        batch_result = batch_run_result

        return batch_result

    def _execute_model(
        self,
        hook_ctx: Any,
        context_config: Any,
        model: ModelNode,
        context: Dict[str, Any],
        materialization_macro: MacroProtocol,
        manifest: Manifest,
    ) -> RunResult:
        try:
            batch_result = self._execute_microbatch_materialization(
                model, context, materialization_macro
            )
        finally:
            self.adapter.post_model_hook(context_config, hook_ctx)

        return batch_result


class MicrobatchModelRunner(ModelRunner):
    """Handles the orchestration of batches to run for a given microbatch model"""

    def __init__(self, config, adapter, node, node_index: int, num_nodes: int):
        super().__init__(config, adapter, node, node_index, num_nodes)

        # The parent task is necessary because we need access to the `_submit_batch` and `submit` methods
        self._parent_task: Optional[RunTask] = None
        # The pool is necessary because we need to batches to be executed within the same thread pool
        self._pool: Optional[DbtThreadPool] = None

    def set_parent_task(self, parent_task: RunTask) -> None:
        self._parent_task = parent_task

    def set_pool(self, pool: DbtThreadPool) -> None:
        self._pool = pool

    @property
    def parent_task(self) -> RunTask:
        if self._parent_task is None:
            raise DbtInternalError(
                msg="Tried to access `parent_task` of `MicrobatchModelRunner` before it was set"
            )

        return self._parent_task

    @property
    def pool(self) -> DbtThreadPool:
        if self._pool is None:
            raise DbtInternalError(
                msg="Tried to access `pool` of `MicrobatchModelRunner` before it was set"
            )

        return self._pool

    def _has_relation(self, model: ModelNode) -> bool:
        """Check whether the relation for the model exists in the data warehouse"""
        relation_info = self.adapter.Relation.create_from(self.config, model)  # type: ignore[arg-type]
        relation = self.adapter.get_relation(
            relation_info.database, relation_info.schema, relation_info.name
        )
        return relation is not None

    def _is_incremental(self, model) -> bool:
        """Check whether the model should be run `incrementally` or as `full refresh`"""
        # TODO: Remove this whole function. This should be a temporary method. We're working with adapters on
        # a strategy to ensure we can access the `is_incremental` logic without drift
        relation_info = self.adapter.Relation.create_from(self.config, model)
        relation = self.adapter.get_relation(
            relation_info.database, relation_info.schema, relation_info.name
        )
        if (
            relation is not None
            and relation.type == "table"
            and model.config.materialized == "incremental"
        ):
            if model.config.full_refresh is not None:
                return not model.config.full_refresh
            else:
                return not getattr(self.config.args, "FULL_REFRESH", False)
        else:
            return False

    def _initial_run_microbatch_model_result(self, model: ModelNode) -> RunResult:
        return RunResult(
            node=model,
            status=RunStatus.Success,
            timing=[],
            thread_id=threading.current_thread().name,
            # The execution_time here doesn't get propagated to logs because
            # `safe_run_hooks` handles the elapsed time at the node level
            execution_time=0,
            message="",
            adapter_response={},
            failures=0,
            batch_results=BatchResults(),
        )

    def describe_node(self) -> str:
        return f"{self.node.language} microbatch model {self.get_node_representation()}"

    def merge_batch_results(self, result: RunResult, batch_results: List[RunResult]):
        """merge batch_results into result"""
        if result.batch_results is None:
            result.batch_results = BatchResults()

        for batch_result in batch_results:
            if batch_result.batch_results is not None:
                result.batch_results += batch_result.batch_results
            result.execution_time += batch_result.execution_time

        num_successes = len(result.batch_results.successful)
        num_failures = len(result.batch_results.failed)
        if num_failures == 0:
            status = RunStatus.Success
            msg = "SUCCESS"
        elif num_successes == 0:
            status = RunStatus.Error
            msg = "ERROR"
        else:
            status = RunStatus.PartialSuccess
            msg = f"PARTIAL SUCCESS ({num_successes}/{num_successes + num_failures})"
        result.status = status
        result.message = msg

        result.batch_results.successful = sorted(result.batch_results.successful)
        result.batch_results.failed = sorted(result.batch_results.failed)

        # # If retrying, propagate previously successful batches into final result, even thoguh they were not run in this invocation
        if self.node.previous_batch_results is not None:
            result.batch_results.successful += self.node.previous_batch_results.successful

    def _update_result_with_unfinished_batches(
        self, result: RunResult, batches: Dict[int, BatchType]
    ) -> None:
        """This method is really only to be used when the execution of a microbatch model is halted before all batches have had a chance to run"""
        batches_finished: Set[BatchType] = set()

        if result.batch_results:
            # build list of finished batches
            batches_finished = batches_finished.union(set(result.batch_results.successful))
            batches_finished = batches_finished.union(set(result.batch_results.failed))
        else:
            # instantiate `batch_results` if it was `None`
            result.batch_results = BatchResults()

        # skipped batches are any batch that was expected but didn't finish
        batches_expected = {batch for _, batch in batches.items()}
        skipped_batches = batches_expected.difference(batches_finished)

        result.batch_results.failed.extend(list(skipped_batches))

        # We call this method, even though we are merging no new results, as it updates
        # the result witht he appropriate status (Success/Partial/Failed)
        self.merge_batch_results(result, [])

    def get_microbatch_builder(self, model: ModelNode) -> MicrobatchBuilder:
        # Intially set the start/end to values from args
        event_time_start = getattr(self.config.args, "EVENT_TIME_START", None)
        event_time_end = getattr(self.config.args, "EVENT_TIME_END", None)

        # If we're in sample mode, alter start/end to sample values
        if getattr(self.config.args, "SAMPLE", None) is not None:
            event_time_start = self.config.args.sample.start
            event_time_end = self.config.args.sample.end

        # During retry, use the original invocation time so that the same
        # batches are recomputed rather than batches relative to "now".
        default_end_time = (
            self.parent_task.original_invocation_started_at or get_invocation_started_at()
        )

        return MicrobatchBuilder(
            model=model,
            is_incremental=self._is_incremental(model),
            event_time_start=event_time_start,
            event_time_end=event_time_end,
            default_end_time=default_end_time,
        )

    def get_batches(self, model: ModelNode) -> Dict[int, BatchType]:
        """Get the batches that should be run for the model"""

        # Note currently (02/23/2025) model.previous_batch_results is only ever _not_ `None`
        # IFF `dbt retry` is being run and the microbatch model had batches which
        # failed on the run of the model (which is being retried)
        if model.previous_batch_results is None:
            microbatch_builder = self.get_microbatch_builder(model)
            end = microbatch_builder.build_end_time()
            start = microbatch_builder.build_start_time(end)
            batches = microbatch_builder.build_batches(start, end)
        else:
            batches = model.previous_batch_results.failed

        return {batch_idx: batches[batch_idx] for batch_idx in range(len(batches))}

    def compile(self, manifest: Manifest):
        """Don't do anything here because this runner doesn't need to compile anything"""
        return self.node

    def execute(self, model: ModelNode, manifest: Manifest) -> RunResult:
        # Execution really means orchestration in this case

        batches = self.get_batches(model=model)
        relation_exists = self._has_relation(model=model)
        result = self._initial_run_microbatch_model_result(model=model)

        # No batches to run, so return initial result
        if len(batches) == 0:
            return result

        batch_results: List[RunResult] = []
        batch_idx = 0

        # Run first batch not in parallel
        relation_exists = self.parent_task._submit_batch(
            node=model,
            adapter=self.adapter,
            relation_exists=relation_exists,
            batches=batches,
            batch_idx=batch_idx,
            batch_results=batch_results,
            pool=self.pool,
            force_sequential_run=True,
            incremental_batch=self._is_incremental(model=model),
        )
        batch_idx += 1
        skip_batches = batch_results[0].status != RunStatus.Success

        # Run all batches except first and last batch, in parallel if possible
        while batch_idx < len(batches) - 1:
            relation_exists = self.parent_task._submit_batch(
                node=model,
                adapter=self.adapter,
                relation_exists=relation_exists,
                batches=batches,
                batch_idx=batch_idx,
                batch_results=batch_results,
                pool=self.pool,
                skip=skip_batches,
            )
            batch_idx += 1

        # Wait until all submitted batches have completed
        while len(batch_results) != batch_idx:
            # Check if the pool was closed, because if it was, then the main thread is trying to exit.
            # If the main thread is trying to exit, we need to shutdown. If we _don't_ shutdown, then
            # batches will continue to execute and we'll delay the run from stopping
            if self.pool.is_closed():
                # It's technically possible for more results to come in while we clean up
                # instead we're going to say the didn't finish, regardless of if they finished
                # or not. Thus, lets get a copy of the results as they exist right "now".
                frozen_batch_results = deepcopy(batch_results)
                self.merge_batch_results(result, frozen_batch_results)
                self._update_result_with_unfinished_batches(result, batches)
                return result

            # breifly sleep so that this thread doesn't go brrrrr while waiting
            time.sleep(0.1)

        # Only run "last" batch if there is more than one batch
        if len(batches) != 1:
            # Final batch runs once all others complete to ensure post_hook runs at the end
            self.parent_task._submit_batch(
                node=model,
                adapter=self.adapter,
                relation_exists=relation_exists,
                batches=batches,
                batch_idx=batch_idx,
                batch_results=batch_results,
                pool=self.pool,
                force_sequential_run=True,
                skip=skip_batches,
            )

        # Finalize run: merge results, track model run, and print final result line
        self.merge_batch_results(result, batch_results)

        pointer_relations: List[BaseRelation] = []
        if result.status == RunStatus.Success and self._should_create_latest_version_pointer(
            model
        ):
            context = generate_runtime_model_context(model, self.config, manifest)
            source_relations = [self.adapter.Relation.create_from(self.config, model)]  # type: ignore[arg-type]
            pointer_relations = self._materialize_latest_version_pointer(
                manifest, model, context, source_relations
            )
        for relation in pointer_relations:
            self.adapter.cache_added(relation.incorporate(dbt_created=True))

        return result


class RunTask(CompileTask):
    def __init__(
        self,
        args: Flags,
        config: RuntimeConfig,
        manifest: Manifest,
        batch_map: Optional[Dict[str, BatchResults]] = None,
        catalogs: Optional[List[Catalog]] = None,
    ) -> None:
        super().__init__(args, config, manifest, catalogs=catalogs)
        self.batch_map = batch_map
        self.overload_map: Optional[Dict[str, OverloadResults]] = None
        self.original_invocation_started_at: Optional[datetime] = None

    def raise_on_first_error(self) -> bool:
        return False

    def get_hook_sql(self, adapter, hook, idx, num_hooks, extra_context) -> str:
        if self.manifest is None:
            raise DbtInternalError("compile_node called before manifest was loaded")

        compiled = self.compiler.compile_node(hook, self.manifest, extra_context)
        statement = compiled.compiled_code
        hook_index = hook.index or num_hooks
        hook_obj = get_hook(statement, index=hook_index)
        return hook_obj.sql or ""

    def handle_job_queue(self, pool: DbtThreadPool, callback: Callable) -> None:
        assert self.job_queue is not None
        node = self.job_queue.get()
        self._raise_set_error()
        runner = self.get_runner(node)
        # we finally know what we're running! Make sure we haven't decided
        # to skip it due to upstream failures
        if runner.node.unique_id in self._skipped_children:
            cause = self._skipped_children.pop(runner.node.unique_id)
            runner.do_skip(cause=cause)

        if isinstance(runner, MicrobatchModelRunner):
            runner.set_parent_task(self)
            runner.set_pool(pool)

        args = [runner]

        # Ensure we don't submit more MicrobatchModelRunners than the pool allows
        self._maybe_wait_for_microbatch(pool, runner)
        self._submit(pool, args, callback)

    def _maybe_wait_for_microbatch(self, pool: DbtThreadPool, runner: BaseRunner) -> None:
        """
        Checks if the runner is a MicrobatchModelRunner, and waits until the
        number of microbatch models in progress is less than the max number
        of microbatch models allowed by the pool.
        """

        if isinstance(runner, MicrobatchModelRunner) and self.job_queue is not None:
            fire_event(
                MicrobatchExecutionDebug(
                    msg=f"Waiting for microbatch model to be run: {runner.node.name}.\n\tpool.max_microbatch_models: {pool.max_microbatch_models}\n\tlen(self.job_queue.in_progress_microbatch): {len(self.job_queue.in_progress_microbatch)}"
                )
            )
            while pool.max_microbatch_models < len(self.job_queue.in_progress_microbatch):
                time.sleep(0.1)

        return

    def _submit_batch(
        self,
        node: ModelNode,
        adapter: BaseAdapter,
        relation_exists: bool,
        batches: Dict[int, BatchType],
        batch_idx: int,
        batch_results: List[RunResult],
        pool: DbtThreadPool,
        force_sequential_run: bool = False,
        skip: bool = False,
        incremental_batch: bool = True,
    ):
        node_copy = deepcopy(node)
        # Only run pre_hook(s) for first batch
        if batch_idx != 0:
            node_copy.config.pre_hook = []

        # Only run post_hook(s) for last batch
        if batch_idx != len(batches) - 1:
            node_copy.config.post_hook = []

        # TODO: We should be doing self.get_runner, however doing so
        # currently causes the tracking of how many nodes there are to
        # increment when we don't want it to
        batch_runner = MicrobatchBatchRunner(
            self.config,
            adapter,
            node_copy,
            self.run_count,
            self.num_nodes,
            batch_idx,
            batches,
            relation_exists,
            incremental_batch,
        )

        if skip:
            batch_runner.do_skip()

        if not pool.is_closed():
            # Only run the batch in parallel IFF:
            # 1. The batch runner is not forced to run sequentially
            # 2. The batch runner should be run in parallel
            # 3. There are available threads in the pool
            #   a. This prevents deadlocks from occurring --threads=1
            if (
                not force_sequential_run
                and batch_runner.should_run_in_parallel()
                and self.job_queue is not None
                and len(self.job_queue.in_progress) < pool.max_threads
            ):
                fire_event(
                    MicrobatchExecutionDebug(
                        msg=f"{batch_runner.describe_batch()} is being run concurrently"
                    )
                )
                self._submit(pool, [batch_runner], batch_results.append)
            else:
                fire_event(
                    MicrobatchExecutionDebug(
                        msg=f"{batch_runner.describe_batch()} is being run sequentially"
                    )
                )
                # MicrobatchBatchRunner always returns RunResult; cast required
                # because call_runner() returns NodeResult after being broadened
                # to accommodate FreshnessRunner. Resolved when NodeT TypeVar is
                # added to BaseRunner so call_runner can return runner.RunnerResultT.
                batch_results.append(cast(RunResult, self.call_runner(batch_runner)))
                relation_exists = batch_runner.relation_exists
        else:
            batch_results.append(
                batch_runner._build_failed_run_batch_result(node_copy, batches[batch_idx])
            )

        return relation_exists

    def _hook_keyfunc(self, hook: HookNode) -> Tuple[str, Optional[int]]:
        package_name = hook.package_name
        if package_name == self.config.project_name:
            package_name = BiggestName("")
        return package_name, hook.index

    def get_hooks_by_type(self, hook_type: RunHookType) -> List[HookNode]:

        if self.manifest is None:
            raise DbtInternalError("self.manifest was None in get_hooks_by_type")

        nodes = self.manifest.nodes.values()
        # find all hooks defined in the manifest (could be multiple projects)
        hooks: List[HookNode] = get_hooks_by_tags(nodes, {hook_type})
        hooks.sort(key=self._hook_keyfunc)
        return hooks

    def safe_run_hooks(
        self, adapter: BaseAdapter, hook_type: RunHookType, extra_context: Dict[str, Any]
    ) -> RunStatus:
        ordered_hooks = self.get_hooks_by_type(hook_type)

        if hook_type == RunHookType.End and ordered_hooks:
            fire_event(Formatting(""))

        # on-run-* hooks should run outside a transaction. This happens because psycopg2 automatically begins a transaction when a connection is created.
        adapter.clear_transaction()
        if not ordered_hooks:
            return RunStatus.Success

        status = RunStatus.Success
        failed = False
        num_hooks = len(ordered_hooks)

        with self._maybe_span(hook_type.value) as hook_span:
            hook_span.set_attribute("hook_type", hook_type.value)
            for idx, hook in enumerate(ordered_hooks, 1):
                with log_contextvars(node_info=hook.node_info), self._maybe_span(
                    hook.unique_id
                ) as hook_node_span:
                    hook.index = idx
                    hook_name = f"{hook.package_name}.{hook_type}.{hook.index - 1}"
                    execution_time = 0.0
                    timing: List[TimingInfo] = []
                    failures = 1

                    if not failed:
                        with collect_timing_info("compile", timing.append):
                            sql = self.get_hook_sql(
                                adapter, hook, hook.index, num_hooks, extra_context
                            )

                        started_at = timing[0].started_at or datetime.now(timezone.utc).replace(
                            tzinfo=None
                        )
                        hook.update_event_status(
                            started_at=started_at.isoformat(), node_status=RunningStatus.Started
                        )

                        fire_event(
                            LogHookStartLine(
                                statement=hook_name,
                                index=hook.index,
                                total=num_hooks,
                                node_info=hook.node_info,
                            )
                        )

                        with collect_timing_info("execute", timing.append):
                            status, message = get_execution_status(sql, adapter)

                        finished_at = timing[1].completed_at or datetime.now(timezone.utc).replace(
                            tzinfo=None
                        )
                        hook.update_event_status(finished_at=finished_at.isoformat())
                        execution_time = (finished_at - started_at).total_seconds()
                        failures = 0 if status == RunStatus.Success else 1

                        if status == RunStatus.Success:
                            message = f"{hook_name} passed"
                        else:
                            message = f"{hook_name} failed, error:\n {message}"
                            failed = True
                            hook_span.set_status(StatusCode.ERROR)
                    else:
                        status = RunStatus.Skipped
                        message = f"{hook_name} skipped"

                    hook.update_event_status(node_status=status)
                    hook_node_span.set_attribute("hook_type", hook_type.value)
                    hook_node_span.set_attribute("package_name", hook.package_name)
                    hook_node_span.set_attribute("name", hook.name)
                    hook_node_span.set_attribute("hook_index", hook.index)
                    hook_node_span.set_attribute("unique_id", hook.unique_id)
                    hook_node_span.set_attribute("hook_outcome", status.value)
                    hook_node_span.set_status(
                        StatusCode.ERROR
                        if status not in (RunStatus.Success, RunStatus.Skipped)
                        else StatusCode.OK
                    )

                    self.node_results.append(
                        RunResult(
                            status=status,
                            thread_id="main",
                            timing=timing,
                            message=message,
                            adapter_response={},
                            execution_time=execution_time,
                            failures=failures,
                            node=hook,
                        )
                    )

                    fire_event(
                        LogHookEndLine(
                            statement=hook_name,
                            status=status,
                            index=hook.index,
                            total=num_hooks,
                            execution_time=execution_time,
                            node_info=hook.node_info,
                        )
                    )

            if not failed:
                # Match the node and per-hook spans, which set OK explicitly rather
                # than leaving a successful span UNSET.
                hook_span.set_status(StatusCode.OK)

        if hook_type == RunHookType.Start and ordered_hooks:
            fire_event(Formatting(""))

        return status

    def print_results_line(self, results, execution_time) -> None:
        nodes = [r.node for r in results if hasattr(r, "node")]
        stat_line = get_counts(nodes)

        execution = ""

        if execution_time is not None:
            execution = utils.humanize_execution_time(execution_time=execution_time)

        fire_event(Formatting(""))
        fire_event(
            FinishedRunningStats(
                stat_line=stat_line, execution=execution, execution_time=execution_time
            )
        )

    def populate_microbatch_batches(self, selected_uids: AbstractSet[str]):
        if self.batch_map is not None and self.manifest is not None:
            for uid in selected_uids:
                if uid in self.batch_map:
                    node = self.manifest.ref_lookup.perform_lookup(uid, self.manifest)
                    if isinstance(node, ModelNode):
                        node.previous_batch_results = self.batch_map[uid]

    def populate_function_overloads(self, selected_uids: AbstractSet[str]):
        if self.overload_map is None or self.manifest is None:
            return
        for uid in selected_uids:
            if uid in self.overload_map:
                node = self.manifest.functions.get(uid)
                if isinstance(node, FunctionNode):
                    node.previous_overload_results = self.overload_map[uid]

    def before_run(self, adapter: BaseAdapter, selected_uids: AbstractSet[str]) -> RunStatus:
        with adapter.connection_named("master"):
            self.defer_to_manifest()
            required_schemas = self.get_model_schemas(adapter, selected_uids)
            with self._maybe_span("metadata.setup"):
                self.create_schemas(adapter, required_schemas)
                self.populate_adapter_cache(adapter, required_schemas)
            self.populate_microbatch_batches(selected_uids)
            self.populate_function_overloads(selected_uids)
            group_lookup.init(self.manifest, selected_uids)
            run_hooks_status = self.safe_run_hooks(adapter, RunHookType.Start, {})
            return run_hooks_status

    def after_run(self, adapter, results) -> None:
        # in on-run-end hooks, provide the value 'database_schemas', which is a
        # list of unique (database, schema) pairs that successfully executed
        # models were in. For backwards compatibility, include the old
        # 'schemas', which did not include database information.

        database_schema_set: Set[Tuple[Optional[str], str]] = {
            (r.node.database, r.node.schema)
            for r in results
            if (hasattr(r, "node") and r.node.is_relational)
            and r.status not in (NodeStatus.Error, NodeStatus.Fail, NodeStatus.Skipped)
        }

        extras = {
            "schemas": list({s for _, s in database_schema_set}),
            "results": [
                r for r in results if r.thread_id != "main" or r.status == RunStatus.Error
            ],  # exclude that didn't fail to preserve backwards compatibility
            "database_schemas": list(database_schema_set),
        }

        try:
            with adapter.connection_named("master"):
                self.safe_run_hooks(adapter, RunHookType.End, extras)
        except (KeyboardInterrupt, SystemExit, DbtRuntimeError):
            run_result = self.get_result(
                results=self.node_results,
                elapsed_time=time.time() - self.started_at,
                generated_at=datetime.now(timezone.utc).replace(tzinfo=None),
            )

            if self.args.write_json and hasattr(run_result, "write"):
                run_result.write(self.result_path())
                add_artifact_produced(self.result_path())

            print_run_end_messages(self.node_results, keyboard_interrupt=True)

            raise

    def get_node_selector(self) -> ResourceTypeSelector:
        if self.manifest is None or self.graph is None:
            raise DbtInternalError("manifest and graph must be set to get perform node selection")
        return ResourceTypeSelector(
            graph=self.graph,
            manifest=self.manifest,
            previous_state=self.previous_state,
            resource_types=[NodeType.Model],
            selectors=self.config.selectors,
        )

    def get_runner_type(self, node) -> Optional[Type[BaseRunner]]:
        if self.manifest is None:
            raise DbtInternalError("manifest must be set prior to calling get_runner_type")

        if (
            node.config.materialized == "incremental"
            and node.config.incremental_strategy == "microbatch"
            and self.manifest.use_microbatch_batches(project_name=self.config.project_name)
        ):
            return MicrobatchModelRunner
        else:
            return ModelRunner

    def task_end_messages(self, results) -> None:
        if results:
            print_run_end_messages(results)
