from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any, Dict, Optional

from dbt.adapters.base.relation import BaseRelation
from dbt.adapters.planning import (
    CreateFromQueryRenderArguments,
    CreateFromQueryRenderKind,
    IncrementalMaterializationPlan,
    IncrementalMaterializationStrategy,
    IncrementalMutationPlan,
    IncrementalPartitionFacts,
    IncrementalRelationFamily,
    IncrementalSchemaChangePlan,
    MaterializationConfig,
    SnapshotMaterializationFacts,
    SnapshotMaterializationPlan,
    SnapshotMaterializationStrategy,
    SnapshotRelationFamily,
    SnapshotStrategyFacts,
    TableMaterializationStrategy,
    TableRelationFamily,
    ViewMaterializationStrategy,
)
from dbt.adapters.planning import (
    MaterializationRuntime as MaterializationRuntimeProtocol,
)
from dbt.contracts.graph.nodes import CompiledNode
from dbt.exceptions import DbtInternalError


@dataclass
class MaterializationRuntime(MaterializationRuntimeProtocol):
    """Typed dbt-core bridge used by adapter-owned lifecycle strategies."""

    adapter: Any
    model: CompiledNode
    context: Dict[str, Any]
    _table_relations: Optional[TableRelationFamily] = field(default=None, init=False)
    _view_relations: Optional[TableRelationFamily] = field(default=None, init=False)
    _incremental_relations: Optional[IncrementalRelationFamily] = field(
        default=None, init=False
    )
    _snapshot_relations: Optional[SnapshotRelationFamily] = field(
        default=None, init=False
    )
    _strategy_macro: Any = field(default=None, init=False)

    @property
    def config(self) -> MaterializationConfig:
        config = self._context_value("config")
        if not callable(getattr(config, "get", None)):
            raise DbtInternalError("Materialization config must provide get()")
        return config

    def resolve_table_strategy(
        self, strategy: TableMaterializationStrategy
    ) -> TableMaterializationStrategy:
        target = self.adapter.resolve_table_materialization_relation(
            self.model, self._context_value("this")
        )
        existing = self.adapter.resolve_table_materialization_existing_relation(target)
        intermediate = self._call_macro("make_intermediate_relation", target)
        backup_type = "table" if existing is None else existing.type
        backup = self._call_macro("make_backup_relation", target, backup_type)
        self._table_relations = TableRelationFamily(
            target=target,
            existing=existing,
            intermediate=intermediate,
            backup=backup,
            preexisting_intermediate=self._call_macro(
                "load_cached_relation", intermediate
            ),
            preexisting_backup=self._call_macro("load_cached_relation", backup),
        )
        facts = self.adapter.build_table_materialization_facts(
            self.model,
            target,
            existing,
        )
        return strategy.resolve(facts)

    def resolve_incremental_strategy(
        self, materialization: IncrementalMaterializationPlan
    ) -> IncrementalMaterializationStrategy:
        target = self._context_value("this").incorporate(type="table")
        existing = self._call_macro("load_cached_relation", self._context_value("this"))
        config = self.config
        catalog_relation = self.adapter.build_catalog_relation(self.model)
        mutation = self.adapter.plan_incremental_mutation(
            config.get("incremental_strategy") or "default",
            language=str(self.model.language),
            unique_key=config.get("unique_key"),
            requested_temp_relation_type=config.get("tmp_relation_type"),
            catalog_relation=catalog_relation,
        )
        self._strategy_macro = self.adapter.get_incremental_plan_macro(
            self.context, mutation
        )
        if not callable(self._strategy_macro):
            raise DbtInternalError(
                "Incremental mutation plan did not resolve to a callable"
            )

        intermediate = self._call_macro("make_intermediate_relation", target)
        backup_type = "table" if existing is None else existing.type
        backup = self._call_macro("make_backup_relation", target, backup_type)
        staging = self._call_macro("make_temp_relation", target)
        if mutation.temp_relation_type is not None:
            object_type = mutation.temp_relation_type.value
            staging = staging.incorporate(
                type="table" if object_type == "transient" else object_type
            )
        self._incremental_relations = IncrementalRelationFamily(
            target=target,
            existing=existing,
            intermediate=intermediate,
            backup=backup,
            staging=staging,
            preexisting_intermediate=self._call_macro(
                "load_cached_relation", intermediate
            ),
            preexisting_backup=self._call_macro("load_cached_relation", backup),
        )

        full_refresh = bool(
            self._call_macro("should_full_refresh")
            or (existing is not None and existing.is_view)
        )
        facts = self.adapter.build_incremental_lifecycle_facts(
            mutation,
            self.model,
            target,
            existing,
            full_refresh=full_refresh,
            on_schema_change=config.get("on_schema_change"),
            contract_enforced=self._contract_enforced(),
        )
        return materialization.resolve(mutation, facts)

    def resolve_view_strategy(
        self, strategy: ViewMaterializationStrategy
    ) -> ViewMaterializationStrategy:
        target = self.adapter.resolve_view_materialization_relation(
            self.model, self._context_value("this")
        )
        existing = self.adapter.resolve_view_materialization_existing_relation(target)
        intermediate = self._call_macro("make_intermediate_relation", target)
        backup_type = "view" if existing is None else existing.type
        backup = self._call_macro("make_backup_relation", target, backup_type)
        self._view_relations = TableRelationFamily(
            target=target,
            existing=existing,
            intermediate=intermediate,
            backup=backup,
            preexisting_intermediate=self._call_macro(
                "load_cached_relation", intermediate
            ),
            preexisting_backup=self._call_macro("load_cached_relation", backup),
        )
        facts = self.adapter.build_view_materialization_facts(
            self.model,
            target,
            existing,
            full_refresh=bool(self._call_macro("should_full_refresh")),
        )
        return strategy.resolve(facts)

    def resolve_snapshot_strategy(
        self, materialization: SnapshotMaterializationPlan
    ) -> SnapshotMaterializationStrategy:
        target_table = getattr(self.model, "alias", None) or self.model.name
        result = self._call_macro(
            "get_or_create_relation",
            database=self.model.database,
            schema=self.model.schema,
            identifier=target_table,
            type="table",
        )
        if (
            isinstance(result, (str, bytes))
            or not isinstance(result, Sequence)
            or len(result) != 2
        ):
            raise DbtInternalError(
                "Snapshot relation resolver must return existence and relation"
            )
        target_exists, target = result
        if not isinstance(target_exists, bool) or not isinstance(target, BaseRelation):
            raise DbtInternalError("Snapshot relation resolver returned invalid values")
        if not target.is_table:
            self.raise_wrong_relation_type(target, "table")

        staging = self._call_macro("make_temp_relation", target)
        if not isinstance(staging, BaseRelation):
            raise DbtInternalError("Snapshot staging resolver must return a relation")
        self._snapshot_relations = SnapshotRelationFamily(
            target=target,
            staging=staging,
            target_exists=target_exists,
        )

        strategy_name = self.config.get("strategy")
        if not isinstance(strategy_name, str) or not strategy_name.strip():
            raise DbtInternalError(
                "Snapshot strategy config must be a non-empty string"
            )
        strategy_macro = self._call_macro("strategy_dispatch", strategy_name)
        if not callable(strategy_macro):
            raise DbtInternalError("Snapshot strategy dispatch must return a callable")
        context_model = self._context_value("model")
        if isinstance(context_model, Mapping):
            legacy_config = context_model.get("config", {})
        else:
            legacy_config = getattr(context_model, "config", {})
        raw_strategy = strategy_macro(
            context_model,
            "snapshotted_data",
            "source_data",
            legacy_config,
            target_exists,
        )
        if not isinstance(raw_strategy, Mapping):
            raise DbtInternalError("Snapshot strategy must return a mapping")
        try:
            strategy = SnapshotStrategyFacts.from_mapping(raw_strategy)
        except (TypeError, ValueError) as exc:
            raise DbtInternalError(f"Invalid snapshot strategy result: {exc}") from exc

        table = self.adapter.build_table_materialization_facts(
            self.model,
            target,
            target if target_exists else None,
        )
        facts = SnapshotMaterializationFacts(
            table=table,
            target_exists=target_exists,
            strategy=strategy,
        )
        return materialization.resolve(facts)

    def table_relations(self) -> TableRelationFamily:
        if self._table_relations is None:
            raise DbtInternalError("Table relation family has not been resolved")
        return self._table_relations

    def view_relations(self) -> TableRelationFamily:
        if self._view_relations is None:
            raise DbtInternalError("View relation family has not been resolved")
        return self._view_relations

    def incremental_relations(
        self, mutation: IncrementalMutationPlan
    ) -> IncrementalRelationFamily:
        if self._incremental_relations is None:
            raise DbtInternalError("Incremental relation family has not been resolved")
        return self._incremental_relations

    def snapshot_relations(self) -> SnapshotRelationFamily:
        if self._snapshot_relations is None:
            raise DbtInternalError("Snapshot relation family has not been resolved")
        return self._snapshot_relations

    def supports_snapshot_materialization(self) -> bool:
        strategy = self.config.get("strategy")
        if strategy not in {"timestamp", "check"}:
            return False
        return (
            self._macro_unique_id(self.context.get("build_snapshot_staging_table"))
            == "macro.dbt.build_snapshot_staging_table"
        )

    def drop_if_exists(self, relation: Optional[BaseRelation]) -> None:
        self._call_macro("drop_relation_if_exists", relation)

    def reload_relation(
        self, relation: Optional[BaseRelation]
    ) -> Optional[BaseRelation]:
        if relation is None:
            return None
        return self._call_macro("load_cached_relation", relation)

    def run_hooks(self, phase: str, *, inside_transaction: bool) -> None:
        self._call_macro(
            "run_hooks",
            self._context_value(f"{phase}_hooks"),
            inside_transaction=inside_transaction,
        )

    def create_from_query(
        self,
        relation: BaseRelation,
        *,
        temporary: bool = False,
        auto_begin: bool = True,
        query: Optional[str] = None,
    ) -> None:
        query = self._context_value("sql") if query is None else query
        plan = self.adapter.plan_create_from_query(temporary, relation, self.model)
        arguments = CreateFromQueryRenderArguments(
            relation_sql=str(
                relation.include(database=not temporary, schema=not temporary)
            ),
            query=query,
            sql_header=self.config.get("sql_header"),
            contract_enforced=self._contract_enforced(),
            legacy_renderer_override=self._legacy_renderer_override(),
        )
        result = self.adapter.resolve_create_from_query_render(plan, arguments)
        if result.kind == CreateFromQueryRenderKind.SQL:
            if not isinstance(result.sql, str):
                raise DbtInternalError("Typed create renderer returned invalid SQL")
            build_sql = result.sql
        elif result.kind == CreateFromQueryRenderKind.LEGACY_MACRO:
            build_sql = self._call_macro(
                "get_create_table_as_sql", temporary, relation, query
            )
        else:
            raise DbtInternalError(f"Unknown create renderer result '{result.kind}'")
        if not isinstance(build_sql, str):
            raise DbtInternalError("Create-from-query renderer must return SQL text")
        self._execute_main(build_sql, auto_begin=auto_begin)

    def create_view_from_query(
        self, relation: BaseRelation, *, auto_begin: bool = True
    ) -> None:
        build_sql = self._call_macro(
            "get_create_view_as_sql",
            relation,
            self._context_value("sql"),
        )
        if not isinstance(build_sql, str):
            raise DbtInternalError("Create-view renderer must return SQL text")
        self._execute_main(build_sql, auto_begin=auto_begin)

    def create_indexes(self, relation: BaseRelation) -> None:
        self._call_macro("create_indexes", relation)

    def drop_indexes(self, relation: BaseRelation) -> None:
        self._call_macro("drop_indexes_on_relation", relation)

    def rename(self, source: BaseRelation, destination: BaseRelation) -> None:
        self.adapter.rename_relation(source, destination)

    def apply_grants(
        self,
        relation: BaseRelation,
        *,
        existing: Optional[BaseRelation],
        full_refresh: bool,
    ) -> None:
        grants = self.config.get("grants") or {}
        if not isinstance(grants, Mapping):
            raise DbtInternalError("Materialization grants config must be a mapping")
        should_revoke = self._call_macro("should_revoke", existing, full_refresh)
        self._call_macro(
            "apply_grants",
            relation,
            grants,
            should_revoke=should_revoke,
        )

    def persist_docs(self, relation: BaseRelation, *, for_columns: bool = True) -> None:
        if for_columns:
            self._call_macro("persist_docs", relation, self._context_value("model"))
        else:
            self._call_macro(
                "persist_docs",
                relation,
                self._context_value("model"),
                for_columns=False,
            )

    def grant_view_access(self, relation: BaseRelation) -> None:
        grant_targets = self.config.get("grant_access_to") or ()
        if isinstance(grant_targets, (str, bytes)) or not isinstance(
            grant_targets, Sequence
        ):
            raise DbtInternalError("View grant_access_to config must be a sequence")
        for grant_target in grant_targets:
            if not isinstance(grant_target, Mapping):
                raise DbtInternalError("Each view grant target must be a mapping")
            self.adapter.grant_access_to(relation, "view", None, grant_target)

    def raise_wrong_relation_type(
        self, relation: BaseRelation, expected_type: str
    ) -> None:
        exceptions = self._context_value("exceptions")
        relation_wrong_type = getattr(exceptions, "relation_wrong_type", None)
        if not callable(relation_wrong_type):
            raise DbtInternalError(
                "Materialization exceptions boundary cannot report a wrong relation type"
            )
        relation_wrong_type(relation, expected_type)

    def build_snapshot_initial_query(self, strategy: SnapshotStrategyFacts) -> str:
        query = self._call_macro(
            "build_snapshot_table",
            strategy.to_macro_dict(),
            self._compiled_query(),
        )
        if not isinstance(query, str):
            raise DbtInternalError(
                "Snapshot initial-query renderer must return SQL text"
            )
        return query

    def build_snapshot_staging_query(
        self, strategy: SnapshotStrategyFacts, target: BaseRelation
    ) -> str:
        query = self._call_macro(
            "snapshot_staging_table",
            strategy.to_macro_dict(),
            self._context_value("sql"),
            target,
        )
        if not isinstance(query, str):
            raise DbtInternalError(
                "Snapshot staging-query renderer must return SQL text"
            )
        return query

    def validate_snapshot_target(
        self, target: BaseRelation, strategy: SnapshotStrategyFacts
    ) -> None:
        columns = self.config.get("snapshot_table_column_names") or self._call_macro(
            "get_snapshot_table_column_names"
        )
        self.adapter.assert_valid_snapshot_target_given_strategy(
            target,
            columns,
            strategy.to_macro_dict(),
        )

    def check_snapshot_time_data_types(self, query: str) -> None:
        self._call_macro("check_time_data_types", query)

    def reconcile_snapshot_columns(
        self,
        staging: BaseRelation,
        target: BaseRelation,
        strategy: SnapshotStrategyFacts,
    ) -> Sequence[str]:
        remove_columns = {
            "dbt_change_type",
            "DBT_CHANGE_TYPE",
            "dbt_unique_key",
            "DBT_UNIQUE_KEY",
        }
        if isinstance(strategy.unique_key, tuple):
            for index in range(1, len(strategy.unique_key) + 1):
                remove_columns.add(f"dbt_unique_key_{index}")
                remove_columns.add(f"DBT_UNIQUE_KEY_{index}")

        missing_columns = tuple(
            column
            for column in self.adapter.get_missing_columns(staging, target)
            if getattr(column, "name", None) not in remove_columns
        )
        self._call_macro("create_columns", target, missing_columns)
        source_columns = tuple(
            column
            for column in self.adapter.get_columns_in_relation(staging)
            if getattr(column, "name", None) not in remove_columns
        )
        return tuple(self.adapter.quote(column.name) for column in source_columns)

    def execute_snapshot_merge(
        self,
        target: BaseRelation,
        staging: BaseRelation,
        insert_columns: Sequence[str],
    ) -> None:
        merge_sql = self._call_macro(
            "snapshot_merge_sql",
            target=target,
            source=staging,
            insert_cols=insert_columns,
        )
        if not isinstance(merge_sql, str):
            raise DbtInternalError("Snapshot merge renderer must return SQL text")
        self._execute_main(merge_sql)

    def post_snapshot(self, staging: BaseRelation) -> None:
        self._call_macro("post_snapshot", staging)

    def commit(self) -> None:
        runtime_adapter = self._context_value("adapter")
        commit = getattr(runtime_adapter, "commit", None)
        if not callable(commit):
            raise DbtInternalError("Materialization adapter boundary cannot commit")
        commit()

    def invoke_setup(self, name: Optional[str]) -> Any:
        return self._call_macro(name) if name is not None else None

    def invoke_teardown(self, name: Optional[str], context: Any) -> None:
        if name is not None:
            self._call_macro(name, context)

    def expand_target_columns(self, source: BaseRelation, target: BaseRelation) -> None:
        self.adapter.expand_target_column_types(
            from_relation=source, to_relation=target
        )

    def process_schema_changes(
        self,
        plan: IncrementalSchemaChangePlan,
        source: BaseRelation,
        target: BaseRelation,
    ) -> Sequence[Any]:
        columns = self._call_macro(
            "process_schema_changes", plan.strategy.value, source, target
        )
        return columns or self.adapter.get_columns_in_relation(target)

    def execute_incremental_mutation(
        self,
        plan: IncrementalMutationPlan,
        relations: IncrementalRelationFamily,
        destination_columns: Sequence[Any],
        partition: Optional[IncrementalPartitionFacts],
    ) -> None:
        if not callable(self._strategy_macro):
            raise DbtInternalError("Incremental strategy macro has not been resolved")
        predicates = self.config.get("predicates") or self.config.get(
            "incremental_predicates"
        )
        adapter_arguments: Dict[str, Any] = {
            "incremental_plan": plan,
            "catalog_relation": self.adapter.build_catalog_relation(self.model),
        }
        if partition is not None:
            adapter_arguments["partition_plan"] = partition.to_dict()
        arguments = self.adapter.plan_incremental_arguments(
            target_relation=relations.target,
            temp_relation=relations.staging,
            unique_key=self.config.get("unique_key"),
            dest_columns=destination_columns,
            incremental_predicates=predicates,
            adapter_arguments=adapter_arguments,
        )
        rendered = self._strategy_macro(arguments.to_macro_dict())
        if isinstance(rendered, str):
            statements = (rendered,)
        elif isinstance(rendered, Sequence) and all(
            isinstance(statement, str) for statement in rendered
        ):
            statements = tuple(rendered)
        else:
            raise DbtInternalError(
                "Incremental renderer must return SQL text or a SQL sequence"
            )
        for statement in statements:
            self._execute_main(statement)

    def insert_from_query(
        self, relation: BaseRelation, partition: IncrementalPartitionFacts
    ) -> None:
        rendered = self.adapter.render_incremental_insert_from_query(
            relation,
            self._context_value("sql"),
            partition,
            self.config.get("sql_header"),
        )
        if not isinstance(rendered, str):
            raise DbtInternalError("Insert-from-query renderer must return SQL text")
        self._execute_main(rendered, auto_begin=False)

    def copy_incremental_partitions(
        self,
        source: BaseRelation,
        target: BaseRelation,
        partition: IncrementalPartitionFacts,
    ) -> None:
        self.adapter.execute_incremental_partition_copy(source, target, partition)

    def _execute_main(self, sql: str, *, auto_begin: bool = True) -> None:
        self._call_macro("write", sql)
        response, table = self.adapter.execute(sql, auto_begin=auto_begin, fetch=False)
        self._call_macro("store_result", "main", response=response, agate_table=table)

    def _contract_enforced(self) -> bool:
        contract = self.config.get("contract")
        if isinstance(contract, Mapping):
            return bool(contract.get("enforced"))
        return bool(getattr(contract, "enforced", False))

    def _compiled_query(self) -> str:
        context_model = self._context_value("model")
        if isinstance(context_model, Mapping):
            compiled = context_model.get("compiled_code")
        else:
            compiled = getattr(context_model, "compiled_code", None)
        if isinstance(compiled, str):
            return compiled
        query = self._context_value("sql")
        if not isinstance(query, str):
            raise DbtInternalError("Materialization SQL must be text")
        return query

    def _context_value(self, name: str) -> Any:
        try:
            return self.context[name]
        except KeyError:
            raise DbtInternalError(
                f"Python materialization context is missing '{name}'"
            ) from None

    def _call_macro(self, name: Optional[str], *args: Any, **kwargs: Any) -> Any:
        if name is None:
            raise DbtInternalError("Materialization macro name cannot be empty")
        macro = self._context_value(name)
        if not callable(macro):
            raise DbtInternalError(
                f"Python materialization context value '{name}' is not callable"
            )
        return macro(*args, **kwargs)

    @staticmethod
    def _macro_unique_id(macro: Any) -> Optional[str]:
        unique_id = getattr(getattr(macro, "macro", None), "unique_id", None)
        return unique_id if isinstance(unique_id, str) else None

    def _legacy_renderer_override(self) -> Optional[str]:
        expected = {
            "get_create_table_as_sql": "macro.dbt.get_create_table_as_sql",
        }
        for name, unique_id in expected.items():
            selected = self._macro_unique_id(self.context.get(name))
            if selected != unique_id:
                return selected or f"unresolved {name} macro"

        wrapper = self.context.get("adapter")
        dispatch = getattr(wrapper, "dispatch", None)
        if not callable(dispatch):
            return "unresolved legacy renderer dispatch"
        dispatched = {
            "get_create_table_as_sql": "macro.dbt.default__get_create_table_as_sql",
            "create_table_as": "macro.dbt.default__create_table_as",
        }
        for name, unique_id in dispatched.items():
            selected = self._macro_unique_id(dispatch(name, "dbt"))
            if selected != unique_id:
                return selected or f"unresolved {name} dispatch"
        return None
