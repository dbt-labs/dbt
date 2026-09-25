from dataclasses import dataclass
from typing import Any, Mapping, Optional

from dbt.adapters.base.relation import BaseRelation
from dbt.contracts.graph.nodes import ModelNode
from dbt.exceptions import DbtInternalError
from dbt.materializations.table import TableMaterializationExecutor


@dataclass(frozen=True)
class IncrementalMaterializationExecutionState:
    """Late-bound relations and a resolved incremental mutation plan."""

    existing_relation: Optional[BaseRelation]
    target_relation: BaseRelation
    intermediate_relation: BaseRelation
    backup_relation: BaseRelation
    temp_relation: BaseRelation
    preexisting_intermediate_relation: Optional[BaseRelation]
    preexisting_backup_relation: Optional[BaseRelation]
    incremental_plan: Any
    strategy_macro: Any
    catalog_relation: Any
    unique_key: Any
    staging_is_temporary: bool
    full_refresh_mode: bool
    on_schema_change: str
    grant_config: Mapping[str, Any]


class IncrementalMaterializationExecutor(TableMaterializationExecutor):
    """Execute the built-in SQL incremental lifecycle in Python."""

    REQUIRED_ADAPTER_METHODS = (
        "build_catalog_relation",
        "get_incremental_plan_macro",
        "plan_incremental_arguments",
        "plan_incremental_mutation",
    )

    def __init__(self, adapter: Any, model: ModelNode, context: dict[str, Any]) -> None:
        super().__init__(adapter, model, context)

    def resolve_incremental_execution_state(self) -> IncrementalMaterializationExecutionState:
        existing_relation = self._call_macro("load_cached_relation", self._context_value("this"))
        target_relation = self._context_value("this").incorporate(type="table")
        intermediate_relation = self._call_macro("make_intermediate_relation", target_relation)
        backup_relation_type = "table" if existing_relation is None else existing_relation.type
        backup_relation = self._call_macro(
            "make_backup_relation", target_relation, backup_relation_type
        )

        config = self._context_value("config")
        unique_key = config.get("unique_key")
        catalog_relation = self.adapter.build_catalog_relation(self.model)
        incremental_plan = self.adapter.plan_incremental_mutation(
            config.get("incremental_strategy") or "default",
            language=str(self.model.language),
            unique_key=unique_key,
            requested_temp_relation_type=config.get("tmp_relation_type"),
            catalog_relation=catalog_relation,
        )
        strategy_macro = self.adapter.get_incremental_plan_macro(self.context, incremental_plan)
        if not callable(strategy_macro):
            raise DbtInternalError("Incremental mutation plan did not resolve to a callable")

        temp_relation = self._call_macro("make_temp_relation", target_relation)
        temp_relation_type = getattr(incremental_plan, "temp_relation_type", None)
        if temp_relation_type is not None:
            object_type = getattr(temp_relation_type, "value", temp_relation_type)
            if object_type == "transient":
                object_type = "table"
            temp_relation = temp_relation.incorporate(type=object_type)

        catalog_staging = getattr(incremental_plan, "catalog_staging", None)
        catalog_staging_value = getattr(catalog_staging, "value", catalog_staging)
        staging_is_temporary = catalog_staging_value != "permanent_table_only"
        full_refresh_mode = bool(
            self._call_macro("should_full_refresh")
            or (existing_relation is not None and existing_relation.is_view)
        )
        on_schema_change = self._call_macro(
            "incremental_validate_on_schema_change",
            config.get("on_schema_change"),
            default="ignore",
        )
        if not isinstance(on_schema_change, str):
            raise DbtInternalError("Incremental schema-change planner must return a string")

        grant_config = config.get("grants") or {}
        if not isinstance(grant_config, Mapping):
            raise DbtInternalError("Incremental materialization grants config must be a mapping")

        return IncrementalMaterializationExecutionState(
            existing_relation=existing_relation,
            target_relation=target_relation,
            intermediate_relation=intermediate_relation,
            backup_relation=backup_relation,
            temp_relation=temp_relation,
            preexisting_intermediate_relation=self._call_macro(
                "load_cached_relation", intermediate_relation
            ),
            preexisting_backup_relation=self._call_macro("load_cached_relation", backup_relation),
            incremental_plan=incremental_plan,
            strategy_macro=strategy_macro,
            catalog_relation=catalog_relation,
            unique_key=unique_key,
            staging_is_temporary=staging_is_temporary,
            full_refresh_mode=full_refresh_mode,
            on_schema_change=on_schema_change,
            grant_config=grant_config,
        )

    def _contract_enforced(self) -> bool:
        contract = self._context_value("config").get("contract")
        if isinstance(contract, Mapping):
            return bool(contract.get("enforced"))
        return bool(getattr(contract, "enforced", False))

    def _incremental_mutation_sql(self, state: IncrementalMaterializationExecutionState) -> str:
        staging_sql = self._build_sql(
            state.temp_relation,
            temporary=state.staging_is_temporary,
        )
        self.adapter.execute(staging_sql, auto_begin=True, fetch=False)

        if not self._contract_enforced():
            self.adapter.expand_target_column_types(
                from_relation=state.temp_relation,
                to_relation=state.target_relation,
            )

        dest_columns = self._call_macro(
            "process_schema_changes",
            state.on_schema_change,
            state.temp_relation,
            state.existing_relation,
        )
        if not dest_columns:
            dest_columns = self.adapter.get_columns_in_relation(state.existing_relation)

        config = self._context_value("config")
        incremental_predicates = config.get("predicates") or config.get("incremental_predicates")
        strategy_arguments = self.adapter.plan_incremental_arguments(
            target_relation=state.target_relation,
            temp_relation=state.temp_relation,
            unique_key=state.unique_key,
            dest_columns=dest_columns,
            incremental_predicates=incremental_predicates,
            adapter_arguments={
                "catalog_relation": state.catalog_relation,
                "incremental_plan": state.incremental_plan,
            },
        )
        build_sql = state.strategy_macro(strategy_arguments.to_macro_dict())
        if not isinstance(build_sql, str):
            raise DbtInternalError("Incremental mutation renderer must return SQL text")
        return build_sql

    def execute(self) -> dict[str, Any]:
        state = self.resolve_incremental_execution_state()

        self._call_macro("drop_relation_if_exists", state.preexisting_intermediate_relation)
        self._call_macro("drop_relation_if_exists", state.preexisting_backup_relation)
        self._call_macro("run_hooks", self._context_value("pre_hooks"), inside_transaction=False)
        self._call_macro("run_hooks", self._context_value("pre_hooks"), inside_transaction=True)

        need_swap = False
        if state.existing_relation is None:
            build_sql = self._build_sql(state.target_relation)
            relation_for_indexes = state.target_relation
        elif state.full_refresh_mode:
            build_sql = self._build_sql(state.intermediate_relation)
            relation_for_indexes = state.intermediate_relation
            need_swap = True
        else:
            build_sql = self._incremental_mutation_sql(state)
            relation_for_indexes = state.temp_relation

        self._execute_main(build_sql)

        if state.existing_relation is None or state.full_refresh_mode:
            self._call_macro("create_indexes", relation_for_indexes)

        if need_swap:
            self.adapter.rename_relation(state.target_relation, state.backup_relation)
            self.adapter.rename_relation(state.intermediate_relation, state.target_relation)

        should_revoke = self._call_macro(
            "should_revoke",
            state.existing_relation,
            state.full_refresh_mode,
        )
        self._call_macro(
            "apply_grants",
            state.target_relation,
            state.grant_config,
            should_revoke=should_revoke,
        )
        self._call_macro("persist_docs", state.target_relation, self._context_value("model"))
        self._call_macro("run_hooks", self._context_value("post_hooks"), inside_transaction=True)
        self._commit()

        if need_swap:
            self.adapter.drop_relation(state.backup_relation)

        self._call_macro("run_hooks", self._context_value("post_hooks"), inside_transaction=False)
        return {"relations": [state.target_relation]}
