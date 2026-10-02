from dataclasses import dataclass
from importlib import import_module
from typing import Any, Dict, Mapping, Optional

from dbt.adapters.base.relation import BaseRelation
from dbt.contracts.graph.nodes import ModelNode
from dbt.exceptions import DbtInternalError


@dataclass(frozen=True)
class TableMaterializationExecutionState:
    """Late-bound relations and configuration for one table replacement."""

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

    def resolve_execution_state(self) -> TableMaterializationExecutionState:
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

        return TableMaterializationExecutionState(
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

    @staticmethod
    def _macro_unique_id(macro: Any) -> Optional[str]:
        unique_id = getattr(getattr(macro, "macro", None), "unique_id", None)
        return unique_id if isinstance(unique_id, str) else None

    def _legacy_renderer_override(self, plan: Any) -> Optional[str]:
        context_macros = (
            ("get_create_table_as_sql", "macro.dbt.get_create_table_as_sql"),
            ("render_create_from_query_plan", "macro.dbt.render_create_from_query_plan"),
        )
        for macro_name, expected_unique_id in context_macros:
            selected_unique_id = self._macro_unique_id(self.context.get(macro_name))
            if selected_unique_id != expected_unique_id:
                return selected_unique_id or f"unresolved {macro_name} macro"

        adapter_wrapper = self.context.get("adapter")
        dispatch = getattr(adapter_wrapper, "dispatch", None)
        if not callable(dispatch):
            return "unresolved legacy renderer dispatch"

        dispatched_macros = (
            ("get_create_table_as_sql", "macro.dbt.default__get_create_table_as_sql"),
            ("render_create_from_query_plan", "macro.dbt.default__render_create_from_query_plan"),
            ("create_table_as", "macro.dbt.default__create_table_as"),
        )
        for macro_name, expected_unique_id in dispatched_macros:
            selected = dispatch(macro_name, "dbt")
            selected_unique_id = self._macro_unique_id(selected)
            if selected_unique_id != expected_unique_id:
                return selected_unique_id or f"unresolved {macro_name} dispatch"

        renderer_macro = getattr(plan, "renderer_macro", None)
        if not isinstance(renderer_macro, str):
            return "unresolved plan renderer"
        selected_renderer = self.context.get(renderer_macro)
        selected_renderer_unique_id = self._macro_unique_id(selected_renderer)
        expected_renderer_unique_id = f"macro.dbt.{renderer_macro}"
        if selected_renderer_unique_id != expected_renderer_unique_id:
            return selected_renderer_unique_id or f"unresolved {renderer_macro} renderer"
        return None

    @staticmethod
    def _render_arguments_type() -> Optional[Any]:
        """Load new adapter contract without raising core's adapter minimum version."""

        try:
            planning = import_module("dbt.adapters.planning")
        except ModuleNotFoundError as exc:
            if exc.name != "dbt.adapters.planning":
                raise
            return None
        return getattr(planning, "CreateFromQueryRenderArguments", None)

    def _legacy_build_sql(self, relation: BaseRelation, sql: str, temporary: bool = False) -> str:
        build_sql = self._call_macro("get_create_table_as_sql", temporary, relation, sql)
        if not isinstance(build_sql, str):
            raise DbtInternalError("Table create-from-query renderer must return SQL text")
        return build_sql

    def _build_sql(self, relation: BaseRelation, temporary: bool = False) -> str:
        sql = self._context_value("sql")
        render_arguments_type = self._render_arguments_type()
        resolve_render = getattr(self.adapter, "resolve_create_from_query_render", None)
        plan_create = getattr(self.adapter, "plan_create_from_query", None)
        if (
            render_arguments_type is None
            or not callable(resolve_render)
            or not callable(plan_create)
        ):
            return self._legacy_build_sql(relation, sql, temporary)

        create_plan = plan_create(temporary, relation, self.model)
        relation_sql = str(relation.include(database=not temporary, schema=not temporary))
        config = self._context_value("config")
        contract = config.get("contract")
        contract_enforced = (
            bool(contract.get("enforced"))
            if isinstance(contract, Mapping)
            else bool(getattr(contract, "enforced", False))
        )
        render_arguments = render_arguments_type(
            relation_sql=relation_sql,
            query=sql,
            sql_header=config.get("sql_header"),
            contract_enforced=contract_enforced,
            legacy_renderer_override=self._legacy_renderer_override(create_plan),
        )
        render_result = resolve_render(create_plan, render_arguments)
        render_kind = getattr(getattr(render_result, "kind", None), "value", None)
        if render_kind == "sql":
            rendered_sql = getattr(render_result, "sql", None)
            if not isinstance(rendered_sql, str):
                raise DbtInternalError("Typed create-from-query renderer returned invalid SQL")
            return rendered_sql
        if render_kind == "legacy_macro":
            renderer_macro = getattr(render_result, "renderer_macro", None)
            if renderer_macro != "get_create_table_as_sql":
                raise DbtInternalError(
                    "Typed create-from-query renderer selected an unknown compatibility macro"
                )
            return self._legacy_build_sql(relation, sql, temporary)
        raise DbtInternalError("Typed create-from-query renderer returned an unknown result kind")

    def execute(self) -> Dict[str, Any]:
        state = self.resolve_execution_state()

        self._call_macro("drop_relation_if_exists", state.preexisting_intermediate_relation)
        self._call_macro("drop_relation_if_exists", state.preexisting_backup_relation)
        self._call_macro("run_hooks", self._context_value("pre_hooks"), inside_transaction=False)
        self._call_macro("run_hooks", self._context_value("pre_hooks"), inside_transaction=True)

        build_sql = self._build_sql(state.intermediate_relation)
        self._execute_main(build_sql)
        self._call_macro("create_indexes", state.intermediate_relation)

        existing_relation = state.existing_relation
        if existing_relation is not None:
            existing_relation = self._call_macro("load_cached_relation", existing_relation)
            if existing_relation is not None:
                self.adapter.rename_relation(existing_relation, state.backup_relation)

        self.adapter.rename_relation(state.intermediate_relation, state.target_relation)
        self._call_macro("run_hooks", self._context_value("post_hooks"), inside_transaction=True)

        should_revoke = self._call_macro(
            "should_revoke", existing_relation, full_refresh_mode=True
        )
        self._call_macro(
            "apply_grants",
            state.target_relation,
            state.grant_config,
            should_revoke=should_revoke,
        )
        self._call_macro("persist_docs", state.target_relation, self._context_value("model"))
        self._commit()
        self._call_macro("drop_relation_if_exists", state.backup_relation)
        self._call_macro("run_hooks", self._context_value("post_hooks"), inside_transaction=False)

        return {"relations": [state.target_relation]}
