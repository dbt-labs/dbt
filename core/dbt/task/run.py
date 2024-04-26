import functools
import threading
import time
from typing import List, Dict, Any, Iterable, Set, Tuple, Optional, AbstractSet

from dbt_common.dataclass_schema import dbtClassMixin

from .compile import CompileRunner, CompileTask

from .printer import (
    print_run_end_messages,
    get_counts,
)
from datetime import timezone, datetime
from dbt import tracking
from dbt import utils
from dbt.adapters.base import BaseRelation
from dbt.cli.flags import Flags
from dbt.clients.jinja import MacroGenerator
from dbt.config.runtime import RuntimeConfig
from dbt.context.providers import generate_runtime_model_context
from dbt.contracts.graph.nodes import HookNode, ResultNode
from dbt.contracts.graph.manifest import Manifest
from dbt.artifacts.schemas.results import NodeStatus, RunStatus, RunningStatus, BaseResult
from dbt.artifacts.schemas.run import RunResult
from dbt.artifacts.resources import Hook
from dbt.exceptions import (
    CompilationError,
    DbtInternalError,
    DbtRuntimeError,
)
from dbt_common.exceptions import DbtValidationError
from dbt.adapters.exceptions import MissingMaterializationError
from dbt.adapters.events.types import (
    DatabaseErrorRunningHook,
    HooksRunning,
    FinishedRunningStats,
)
from dbt_common.events.contextvars import log_contextvars
from dbt_common.events.functions import fire_event, get_invocation_id
from dbt_common.events.types import Formatting
from dbt_common.events.base_types import EventLevel
from dbt.events.types import (
    LogModelResult,
    LogStartLine,
    LogHookEndLine,
    LogHookStartLine,
)
from dbt.logger import (
    TextOnly,
    HookMetadata,
    UniqueID,
    TimestampNamed,
    DbtModelState,
)
from dbt.graph import ResourceTypeSelector
from dbt.hooks import get_hook_dict
from dbt.node_types import NodeType, RunHookType


class Timer:
    def __init__(self) -> None:
        self.start = None
        self.end = None

    @property
    def elapsed(self):
        if self.start is None or self.end is None:
            return None
        return self.end - self.start

    def __enter__(self):
        self.start = time.time()
        return self

    def __exit__(self, exc_type, exc_value, exc_tracebck):
        self.end = time.time()


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


def track_model_run(index, num_nodes, run_model_result):
    if tracking.active_user is None:
        raise DbtInternalError("cannot track model run with no active user")
    invocation_id = get_invocation_id()
    node = run_model_result.node
    has_group = True if hasattr(node, "group") and node.group else False
    if node.resource_type == NodeType.Model:
        access = node.access.value if node.access is not None else None
        contract_enforced = node.contract.enforced
        versioned = True if node.version else False
    else:
        access = None
        contract_enforced = False
        versioned = False
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
            "model_id": utils.get_hash(node),
            "hashed_contents": utils.get_hashed_contents(node),
            "timing": [t.to_dict(omit_none=True) for t in run_model_result.timing],
            "language": str(node.language),
            "has_group": has_group,
            "contract_enforced": contract_enforced,
            "access": access,
            "versioned": versioned,
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


class ModelRunner(CompileRunner):
    def get_node_representation(self):
        display_quote_policy = {"database": False, "schema": False, "identifier": False}
        relation = self.adapter.Relation.create_from(
            self.config, self.node, quote_policy=display_quote_policy
        )
        # exclude the database from output if it's the default
        if self.node.database == self.config.credentials.database:
            relation = relation.include(database=False)
        return str(relation)

    def describe_node(self):
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
            ),
            level=level,
        )

    def before_execute(self):
        self.print_start_line()

    def after_execute(self, result):
        track_model_run(self.node_index, self.num_nodes, result)
        self.print_result_line(result)

    def _build_run_model_result(self, model, context):
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
            execution_time=0,
            message=str(result.response),
            adapter_response=adapter_response,
            failures=result.get("failures"),
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

    def execute(self, model, manifest):
        context = generate_runtime_model_context(model, self.config, manifest)

        materialization_macro = manifest.find_materialization_macro_by_name(
            self.config.project_name, model.get_materialization(), self.adapter.type()
        )

        if materialization_macro is None:
            raise MissingMaterializationError(
                materialization=model.get_materialization(), adapter_type=self.adapter.type()
            )

        if "config" not in context:
            raise DbtInternalError(
                "Invalid materialization context generated, missing config: {}".format(context)
            )
        context_config = context["config"]

        mat_has_supported_langs = hasattr(materialization_macro, "supported_languages")
        model_lang_supported = model.language in materialization_macro.supported_languages
        if mat_has_supported_langs and not model_lang_supported:
            str_langs = [str(lang) for lang in materialization_macro.supported_languages]
            raise DbtValidationError(
                f'Materialization "{materialization_macro.name}" only supports languages {str_langs}; '
                f'got "{model.language}"'
            )

        hook_ctx = self.adapter.pre_model_hook(context_config)
        try:
            result = MacroGenerator(
                materialization_macro, context, stack=context["context_macro_stack"]
            )()
        finally:
            self.adapter.post_model_hook(context_config, hook_ctx)

        for relation in self._materialization_relations(result, model):
            self.adapter.cache_added(relation.incorporate(dbt_created=True))

        return self._build_run_model_result(model, context)


class RunTask(CompileTask):
    def __init__(self, args: Flags, config: RuntimeConfig, manifest: Manifest) -> None:
        super().__init__(args, config, manifest)
        self.ran_hooks: List[HookNode] = []
        self._total_executed = 0

    def index_offset(self, value: int) -> int:
        return self._total_executed + value

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

    def run_hooks(self, adapter, hook_type: RunHookType, extra_context) -> None:
        ordered_hooks = self.get_hooks_by_type(hook_type)

        # on-run-* hooks should run outside of a transaction. This happens
        # b/c psycopg2 automatically begins a transaction when a connection
        # is created.
        adapter.clear_transaction()
        if not ordered_hooks:
            return
        num_hooks = len(ordered_hooks)

        with TextOnly():
            fire_event(Formatting(""))
        fire_event(HooksRunning(num_hooks=num_hooks, hook_type=hook_type))

        startctx = TimestampNamed("node_started_at")
        finishctx = TimestampNamed("node_finished_at")

        for idx, hook in enumerate(ordered_hooks, start=1):
            # We want to include node_info in the appropriate log files, so use
            # log_contextvars
            with log_contextvars(node_info=hook.node_info):
                hook.update_event_status(
                    started_at=datetime.now(timezone.utc).replace(tzinfo=None).isoformat(),
                    node_status=RunningStatus.Started,
                )
                sql = self.get_hook_sql(adapter, hook, idx, num_hooks, extra_context)

                hook_text = "{}.{}.{}".format(hook.package_name, hook_type, hook.index)
                hook_meta_ctx = HookMetadata(hook, self.index_offset(idx))
                with UniqueID(hook.unique_id):
                    with hook_meta_ctx, startctx:
                        fire_event(
                            LogHookStartLine(
                                statement=hook_text,
                                index=idx,
                                total=num_hooks,
                                node_info=hook.node_info,
                            )
                        )

                    with Timer() as timer:
                        if len(sql.strip()) > 0:
                            response, _ = adapter.execute(sql, auto_begin=False, fetch=False)
                            status = response._message
                        else:
                            status = "OK"

                    self.ran_hooks.append(hook)
                    hook.update_event_status(
                        finished_at=datetime.now(timezone.utc).replace(tzinfo=None).isoformat()
                    )
                    with finishctx, DbtModelState({"node_status": "passed"}):
                        hook.update_event_status(node_status=RunStatus.Success)
                        fire_event(
                            LogHookEndLine(
                                statement=hook_text,
                                status=status,
                                index=idx,
                                total=num_hooks,
                                execution_time=timer.elapsed,
                                node_info=hook.node_info,
                            )
                        )
                # `_event_status` dict is only used for logging.  Make sure
                # it gets deleted when we're done with it
                hook.clear_event_status()

        self._total_executed += len(ordered_hooks)

        with TextOnly():
            fire_event(Formatting(""))

    def safe_run_hooks(
        self, adapter, hook_type: RunHookType, extra_context: Dict[str, Any]
    ) -> None:
        try:
            self.run_hooks(adapter, hook_type, extra_context)
        except DbtRuntimeError as exc:
            fire_event(DatabaseErrorRunningHook(hook_type=hook_type.value))
            self.node_results.append(
                BaseResult(
                    status=RunStatus.Error,
                    thread_id="main",
                    timing=[],
                    message=f"{hook_type.value} failed, error:\n {exc.msg}",
                    adapter_response={},
                    execution_time=0,
                    failures=1,
                )
            )

    def print_results_line(self, results, execution_time) -> None:
        nodes = [r.node for r in results if hasattr(r, "node")] + self.ran_hooks
        stat_line = get_counts(nodes)

        execution = ""

        if execution_time is not None:
            execution = utils.humanize_execution_time(execution_time=execution_time)

        with TextOnly():
            fire_event(Formatting(""))
        fire_event(
            FinishedRunningStats(
                stat_line=stat_line, execution=execution, execution_time=execution_time
            )
        )

    def before_run(self, adapter, selected_uids: AbstractSet[str]) -> None:
        with adapter.connection_named("master"):
            required_schemas = self.get_model_schemas(adapter, selected_uids)
            self.create_schemas(adapter, required_schemas)
            self.populate_adapter_cache(adapter, required_schemas)
            self.defer_to_manifest(adapter, selected_uids)
            self.safe_run_hooks(adapter, RunHookType.Start, {})

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

        self._total_executed += len(results)

        extras = {
            "schemas": list({s for _, s in database_schema_set}),
            "results": results,
            "database_schemas": list(database_schema_set),
        }
        with adapter.connection_named("master"):
            self.safe_run_hooks(adapter, RunHookType.End, extras)

    def get_node_selector(self) -> ResourceTypeSelector:
        if self.manifest is None or self.graph is None:
            raise DbtInternalError("manifest and graph must be set to get perform node selection")
        return ResourceTypeSelector(
            graph=self.graph,
            manifest=self.manifest,
            previous_state=self.previous_state,
            resource_types=[NodeType.Model],
        )

    def get_runner_type(self, _):
        return ModelRunner

    def task_end_messages(self, results) -> None:
        if results:
            print_run_end_messages(results)
