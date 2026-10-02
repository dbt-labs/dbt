from dataclasses import dataclass
from typing import Any, Dict, Mapping, Optional

from dbt.adapters.base.relation import BaseRelation
from dbt.contracts.graph.nodes import ModelNode
from dbt.exceptions import DbtInternalError


@dataclass(frozen=True)
class TableMaterializationPlan:
    """Resolved relations and configuration for one table replacement."""

    existing_relation: Optional[BaseRelation]
    target_relation: BaseRelation
    intermediate_relation: BaseRelation
    backup_relation: BaseRelation
    preexisting_intermediate_relation: Optional[BaseRelation]
    preexisting_backup_relation: Optional[BaseRelation]
    grant_config: Mapping[str, Any]


class TableMaterializationExecutor:
    """Execute built-in table lifecycle in Python.

    Project and adapter macros remain callable at explicit compatibility boundaries,
    but control flow and database mutation ordering live here rather than in Jinja.
    """

    def __init__(self, adapter: Any, model: ModelNode, context: Dict[str, Any]) -> None:
        self.adapter = adapter
        self.model = model
        self.context = context

    def _context_value(self, name: str) -> Any:
        try:
            return self.context[name]
        except KeyError:
            raise DbtInternalError(
                f"Python table materialization context is missing '{name}'"
            ) from None

    def _call_macro(self, name: str, *args: Any, **kwargs: Any) -> Any:
        macro = self._context_value(name)
        if not callable(macro):
            raise DbtInternalError(
                f"Python table materialization context value '{name}' is not callable"
            )
        return macro(*args, **kwargs)

    def plan(self) -> TableMaterializationPlan:
        existing_relation = self._call_macro("load_cached_relation", self._context_value("this"))
        target_relation = self._context_value("this").incorporate(type="table")
        intermediate_relation = self._call_macro("make_intermediate_relation", target_relation)
        backup_relation_type = "table" if existing_relation is None else existing_relation.type
        backup_relation = self._call_macro(
            "make_backup_relation", target_relation, backup_relation_type
        )

        grant_config = self._context_value("config").get("grants") or {}
        if not isinstance(grant_config, Mapping):
            raise DbtInternalError("Table materialization grants config must be a mapping")

        return TableMaterializationPlan(
            existing_relation=existing_relation,
            target_relation=target_relation,
            intermediate_relation=intermediate_relation,
            backup_relation=backup_relation,
            preexisting_intermediate_relation=self._call_macro(
                "load_cached_relation", intermediate_relation
            ),
            preexisting_backup_relation=self._call_macro("load_cached_relation", backup_relation),
            grant_config=grant_config,
        )

    def _execute_main(self, sql: str) -> None:
        self._call_macro("write", sql)
        response, table = self.adapter.execute(sql, auto_begin=True, fetch=False)
        self._call_macro("store_result", "main", response=response, agate_table=table)

    def _commit(self) -> None:
        runtime_adapter = self._context_value("adapter")
        commit = getattr(runtime_adapter, "commit", None)
        if not callable(commit):
            raise DbtInternalError("Python materialization adapter boundary cannot commit")
        commit()

    def execute(self) -> Dict[str, Any]:
        plan = self.plan()

        self._call_macro("drop_relation_if_exists", plan.preexisting_intermediate_relation)
        self._call_macro("drop_relation_if_exists", plan.preexisting_backup_relation)
        self._call_macro("run_hooks", self._context_value("pre_hooks"), inside_transaction=False)
        self._call_macro("run_hooks", self._context_value("pre_hooks"), inside_transaction=True)

        build_sql = self._call_macro(
            "get_create_table_as_sql",
            False,
            plan.intermediate_relation,
            self._context_value("sql"),
        )
        if not isinstance(build_sql, str):
            raise DbtInternalError("Table create-from-query renderer must return SQL text")
        self._execute_main(build_sql)
        self._call_macro("create_indexes", plan.intermediate_relation)

        existing_relation = plan.existing_relation
        if existing_relation is not None:
            existing_relation = self._call_macro("load_cached_relation", existing_relation)
            if existing_relation is not None:
                self.adapter.rename_relation(existing_relation, plan.backup_relation)

        self.adapter.rename_relation(plan.intermediate_relation, plan.target_relation)
        self._call_macro("run_hooks", self._context_value("post_hooks"), inside_transaction=True)

        should_revoke = self._call_macro(
            "should_revoke", existing_relation, full_refresh_mode=True
        )
        self._call_macro(
            "apply_grants",
            plan.target_relation,
            plan.grant_config,
            should_revoke=should_revoke,
        )
        self._call_macro("persist_docs", plan.target_relation, self._context_value("model"))
        self._commit()
        self._call_macro("drop_relation_if_exists", plan.backup_relation)
        self._call_macro("run_hooks", self._context_value("post_hooks"), inside_transaction=False)

        return {"relations": [plan.target_relation]}
