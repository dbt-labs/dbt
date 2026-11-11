import os
import threading
import time
from typing import AbstractSet, Any, Dict, List, Optional, Type

from dbt import deprecations
from dbt.adapters.base import BaseAdapter
from dbt.adapters.base.impl import FreshnessResponse
from dbt.adapters.base.relation import BaseRelation
from dbt.adapters.capability import Capability
from dbt.adapters.contracts.connection import AdapterResponse
from dbt.artifacts.schemas.freshness import (
    FreshnessNodeResult,
    FreshnessResult,
    FreshnessStatus,
    PartialSourceFreshnessResult,
    SourceFreshnessResult,
)
from dbt.clients import jinja
from dbt.constants import SOURCE_RESULT_FILE_NAME
from dbt.context.providers import RuntimeProvider, SourceContext
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.nodes import HookNode, SourceDefinition
from dbt.contracts.results import RunStatus
from dbt.events.types import FreshnessCheckComplete, LogFreshnessResult, LogStartLine
from dbt.graph import ResourceTypeSelector
from dbt.node_types import NodeType, RunHookType
from dbt_common.events.base_types import EventLevel
from dbt_common.events.functions import fire_event
from dbt_common.events.types import Note
from dbt_common.exceptions import DbtInternalError, DbtRuntimeError

from .base import BaseRunner
from .printer import print_run_result_error
from .run import RunTask


class FreshnessRunner(BaseRunner[SourceDefinition, FreshnessNodeResult]):
    def __init__(self, config, adapter, node, node_index, num_nodes) -> None:
        super().__init__(config, adapter, node, node_index, num_nodes)
        self._metadata_freshness_cache: Dict[BaseRelation, FreshnessResponse] = {}

    def set_metadata_freshness_cache(
        self, metadata_freshness_cache: Dict[BaseRelation, FreshnessResponse]
    ) -> None:
        self._metadata_freshness_cache = metadata_freshness_cache

    def on_skip(self) -> FreshnessNodeResult:
        raise DbtRuntimeError("Freshness: nodes cannot be skipped!")

    def before_execute(self) -> None:
        description = f"freshness of {self.node.source_name}.{self.node.name}"
        fire_event(
            LogStartLine(
                description=description,
                index=self.node_index,
                total=self.num_nodes,
                node_info=self.node.node_info,
            )
        )

    def after_execute(self, result: FreshnessNodeResult) -> None:
        # self.node is SourceDefinition so source_name is available directly
        source_name = self.node.source_name
        table_name = self.node.name
        level = LogFreshnessResult.status_to_level(str(result.status))
        fire_event(
            LogFreshnessResult(
                status=result.status,
                source_name=source_name,
                table_name=table_name,
                index=self.node_index,
                total=self.num_nodes,
                execution_time=result.execution_time,
                node_info=self.node.node_info,
            ),
            level=level,
        )

    def error_result(self, node, message, start_time, timing_info) -> FreshnessNodeResult:
        return self._build_run_result(
            node=node,
            start_time=start_time,
            status=FreshnessStatus.RuntimeErr,
            timing_info=timing_info,
            message=message,
        )

    def _build_run_result(  # type: ignore[override]
        self, node, start_time, status, timing_info, message
    ) -> FreshnessNodeResult:
        execution_time = time.time() - start_time
        thread_id = threading.current_thread().name
        return PartialSourceFreshnessResult(
            status=status,
            thread_id=thread_id,
            execution_time=execution_time,
            timing=timing_info,
            message=message,
            node=node,
            adapter_response={},
            failures=None,
        )

    def from_run_result(
        self, result: FreshnessNodeResult, start_time, timing_info
    ) -> FreshnessNodeResult:
        result.execution_time = time.time() - start_time
        result.timing.extend(timing_info)
        return result

    def execute(self, compiled_node, manifest) -> FreshnessNodeResult:
        relation = self.adapter.Relation.create_from(self.config, compiled_node)
        # given a Source, calculate its freshness.
        with self.adapter.connection_named(compiled_node.unique_id, compiled_node):
            self.adapter.clear_transaction()
            adapter_response: Optional[AdapterResponse] = None
            freshness: Optional[FreshnessResponse] = None

            if compiled_node.loaded_at_query is not None:
                # within the context user can have access to `this`, `source_node`(`model` will point to the same thing),  etc
                compiled_code = jinja.get_rendered(
                    compiled_node.loaded_at_query,
                    SourceContext(
                        compiled_node, self.config, manifest, RuntimeProvider(), None
                    ).to_dict(),
                    compiled_node,
                )
                adapter_response, freshness = self.adapter.calculate_freshness_from_custom_sql(
                    relation,
                    compiled_code,
                    macro_resolver=manifest,
                )
                status = compiled_node.freshness.status(freshness["age"])
            elif compiled_node.loaded_at_field is not None:
                adapter_response, freshness = self.adapter.calculate_freshness(
                    relation,
                    compiled_node.loaded_at_field,
                    compiled_node.freshness.filter,
                    macro_resolver=manifest,
                )

                status = compiled_node.freshness.status(freshness["age"])
            elif self.adapter.supports(Capability.TableLastModifiedMetadata):
                if compiled_node.freshness.filter is not None:
                    fire_event(
                        Note(
                            msg=f"A filter cannot be applied to a metadata freshness check on source '{compiled_node.name}'."
                        ),
                        EventLevel.WARN,
                    )

                metadata_source = self.adapter.Relation.create_from(self.config, compiled_node)
                if metadata_source in self._metadata_freshness_cache:
                    freshness = self._metadata_freshness_cache[metadata_source]
                else:
                    adapter_response, freshness = self.adapter.calculate_freshness_from_metadata(
                        relation,
                        macro_resolver=manifest,
                    )

                status = compiled_node.freshness.status(freshness["age"])
            else:
                raise DbtRuntimeError(
                    f"Could not compute freshness for source {compiled_node.name}: no 'loaded_at_field' provided and {self.adapter.type()} adapter does not support metadata-based freshness checks."
                )
        # adapter_response was not returned in previous versions, so this will be None
        # we cannot call to_dict() on NoneType
        adapter_response_dict: Dict[str, Any] = (
            adapter_response.to_dict(omit_none=True) if adapter_response else {}
        )

        return SourceFreshnessResult(
            node=compiled_node,
            status=status,
            thread_id=threading.current_thread().name,
            timing=[],
            execution_time=0,
            message=None,
            adapter_response=adapter_response_dict,
            failures=None,
            **freshness,
        )

    def compile(self, manifest: Manifest):
        if self.node.resource_type != NodeType.Source:
            # should be unreachable...
            raise DbtRuntimeError("freshness runner: got a non-Source")
        # we don't do anything interesting when we compile a source node
        return self.node


class FreshnessSelector(ResourceTypeSelector):
    def node_is_match(self, node):
        if not super().node_is_match(node):
            return False
        if not isinstance(node, SourceDefinition):
            return False
        return node.has_freshness


class FreshnessTask(RunTask):
    def __init__(self, args, config, manifest, catalogs) -> None:
        super().__init__(args, config, manifest, catalogs)

        if self.args.output:
            deprecations.warn(
                "custom-output-path-in-source-freshness-deprecation", path=str(self.args.output)
            )

        self._metadata_freshness_cache: Dict[BaseRelation, FreshnessResponse] = {}

    def result_path(self) -> str:
        if self.args.output:
            return os.path.realpath(self.args.output)
        else:
            return os.path.join(self.config.project_target_path, SOURCE_RESULT_FILE_NAME)

    def raise_on_first_error(self) -> bool:
        return False

    def get_node_selector(self):
        if self.manifest is None or self.graph is None:
            raise DbtInternalError("manifest and graph must be set to get perform node selection")
        return FreshnessSelector(
            graph=self.graph,
            manifest=self.manifest,
            previous_state=self.previous_state,
            resource_types=[NodeType.Source],
            selectors=self.config.selectors,
        )

    def before_run(self, adapter: BaseAdapter, selected_uids: AbstractSet[str]) -> RunStatus:
        populate_metadata_freshness_cache_status = RunStatus.Success

        before_run_status = super().before_run(adapter, selected_uids)

        if before_run_status == RunStatus.Success and adapter.supports(
            Capability.TableLastModifiedMetadataBatch
        ):
            populate_metadata_freshness_cache_status = self.populate_metadata_freshness_cache(
                adapter, selected_uids
            )

        if (
            before_run_status == RunStatus.Success
            and populate_metadata_freshness_cache_status == RunStatus.Success
        ):
            return RunStatus.Success
        else:
            return RunStatus.Error

    def get_runner(self, node) -> BaseRunner:
        freshness_runner = super().get_runner(node)
        assert isinstance(freshness_runner, FreshnessRunner)
        freshness_runner.set_metadata_freshness_cache(self._metadata_freshness_cache)
        return freshness_runner

    def get_runner_type(self, _) -> Optional[Type[BaseRunner]]:
        return FreshnessRunner

    def get_result(self, results, elapsed_time, generated_at):
        return FreshnessResult.from_node_results(
            elapsed_time=elapsed_time, generated_at=generated_at, results=results
        )

    def task_end_messages(self, results) -> None:
        for result in results:
            if result.status in (
                FreshnessStatus.Error,
                FreshnessStatus.RuntimeErr,
                RunStatus.Error,
            ):
                print_run_result_error(result)

        fire_event(FreshnessCheckComplete())

    def get_hooks_by_type(self, hook_type: RunHookType) -> List[HookNode]:
        hooks = super().get_hooks_by_type(hook_type)
        if self.args.source_freshness_run_project_hooks:
            return hooks
        else:
            if hooks:
                deprecations.warn("source-freshness-project-hooks")
            return []

    def populate_metadata_freshness_cache(
        self, adapter, selected_uids: AbstractSet[str]
    ) -> RunStatus:
        if self.manifest is None:
            raise DbtInternalError("Manifest must be set to populate metadata freshness cache")

        batch_metadata_sources: List[BaseRelation] = []
        for selected_source_uid in list(selected_uids):
            source = self.manifest.sources.get(selected_source_uid)
            if source and source.loaded_at_field is None:
                metadata_source = adapter.Relation.create_from(self.config, source)
                batch_metadata_sources.append(metadata_source)

        fire_event(
            Note(
                msg=f"Pulling freshness from warehouse metadata tables for {len(batch_metadata_sources)} sources"
            ),
            EventLevel.INFO,
        )

        try:
            _, metadata_freshness_results = adapter.calculate_freshness_from_metadata_batch(
                batch_metadata_sources
            )
            self._metadata_freshness_cache.update(metadata_freshness_results)
            return RunStatus.Success
        except Exception as e:
            # This error handling is intentionally very coarse.
            # If anything goes wrong during batch metadata calculation, we can safely
            # leave _metadata_freshness_cache unpopulated.
            # Downstream, this will be gracefully handled as a cache miss and non-batch
            # metadata-based freshness will still be performed on a source-by-source basis.
            fire_event(
                Note(msg=f"Metadata freshness could not be computed in batch: {e}"),
                EventLevel.WARN,
            )
            # Returning Error here would make before_run skip every node when
            # skip_nodes_if_on_run_start_fails is set, instead of falling back.
            return RunStatus.Success

    def get_freshness_metadata_cache(self) -> Dict[BaseRelation, FreshnessResponse]:
        return self._metadata_freshness_cache
