from types import SimpleNamespace
from unittest.mock import MagicMock, call

from dbt.materializations.incremental.executor import (
    IncrementalMaterializationExecutionState,
    IncrementalMaterializationExecutor,
)


def _relation(name, relation_type="table", *, is_view=False):
    relation = MagicMock(name=name)
    relation.name = name
    relation.type = relation_type
    relation.is_view = is_view
    relation.include.return_value = f'"analytics"."{name}"'
    return relation


def _context(existing_relation):
    this = MagicMock(name="this")
    target = _relation("target")
    intermediate = _relation("intermediate")
    backup = _relation("backup")
    temp = _relation("temp")
    this.incorporate.return_value = target
    context = {
        "adapter": MagicMock(),
        "this": this,
        "config": {
            "unique_key": "id",
            "incremental_strategy": "delete+insert",
            "tmp_relation_type": "view",
            "on_schema_change": "append_new_columns",
            "grants": {"select": ["reporter"]},
            "predicates": ["id > 0"],
            "contract": {"enforced": False},
        },
        "model": {"unique_id": "model.test.orders"},
        "sql": "select 1 as id",
        "pre_hooks": ["pre"],
        "post_hooks": ["post"],
        "load_cached_relation": MagicMock(side_effect=[existing_relation, None, None]),
        "make_intermediate_relation": MagicMock(return_value=intermediate),
        "make_backup_relation": MagicMock(return_value=backup),
        "make_temp_relation": MagicMock(return_value=temp),
        "should_full_refresh": MagicMock(return_value=False),
        "incremental_validate_on_schema_change": MagicMock(return_value="append_new_columns"),
        "drop_relation_if_exists": MagicMock(),
        "run_hooks": MagicMock(),
        "process_schema_changes": MagicMock(return_value=["id"]),
        "write": MagicMock(),
        "store_result": MagicMock(),
        "create_indexes": MagicMock(),
        "should_revoke": MagicMock(return_value=False),
        "apply_grants": MagicMock(),
        "persist_docs": MagicMock(),
    }
    return context, target, intermediate, backup, temp


def _model():
    return SimpleNamespace(language="sql")


def test_incremental_executor_plans_before_constructing_typed_staging_relation():
    existing = _relation("existing")
    context, target, intermediate, backup, temp = _context(existing)
    typed_temp = _relation("typed_temp", "view")
    temp.incorporate.return_value = typed_temp
    incremental_plan = SimpleNamespace(
        temp_relation_type=SimpleNamespace(value="view"),
        catalog_staging=SimpleNamespace(value="permanent_table_only"),
    )
    strategy_macro = MagicMock()
    catalog_relation = object()
    adapter = MagicMock()
    adapter.build_catalog_relation.return_value = catalog_relation
    adapter.plan_incremental_mutation.return_value = incremental_plan
    adapter.get_incremental_plan_macro.return_value = strategy_macro

    state = IncrementalMaterializationExecutor(
        adapter, _model(), context
    ).resolve_incremental_execution_state()

    adapter.plan_incremental_mutation.assert_called_once_with(
        "delete+insert",
        language="sql",
        unique_key="id",
        requested_temp_relation_type="view",
        catalog_relation=catalog_relation,
    )
    temp.incorporate.assert_called_once_with(type="view")
    assert state.temp_relation is typed_temp
    assert state.staging_is_temporary is False
    assert state.incremental_plan is incremental_plan
    assert state.strategy_macro is strategy_macro
    assert state.target_relation is target
    assert state.intermediate_relation is intermediate
    assert state.backup_relation is backup


def test_incremental_executor_builds_staging_then_mutation_from_typed_arguments(
    monkeypatch,
):
    existing = _relation("existing")
    context, target, intermediate, backup, temp = _context(existing)
    strategy_macro = MagicMock(return_value="delete from target; insert into target")
    state = IncrementalMaterializationExecutionState(
        existing_relation=existing,
        target_relation=target,
        intermediate_relation=intermediate,
        backup_relation=backup,
        temp_relation=temp,
        preexisting_intermediate_relation=None,
        preexisting_backup_relation=None,
        incremental_plan=SimpleNamespace(),
        strategy_macro=strategy_macro,
        catalog_relation=object(),
        unique_key="id",
        staging_is_temporary=True,
        full_refresh_mode=False,
        on_schema_change="append_new_columns",
        grant_config={},
    )
    strategy_arguments = MagicMock()
    strategy_arguments.to_macro_dict.return_value = {"target_relation": target}
    adapter = MagicMock()
    adapter.plan_incremental_arguments.return_value = strategy_arguments
    executor = IncrementalMaterializationExecutor(adapter, _model(), context)
    monkeypatch.setattr(executor, "_build_sql", MagicMock(return_value="create temp view"))

    build_sql = executor._incremental_mutation_sql(state)

    assert build_sql == "delete from target; insert into target"
    executor._build_sql.assert_called_once_with(temp, temporary=True)
    adapter.execute.assert_called_once_with("create temp view", auto_begin=True, fetch=False)
    adapter.expand_target_column_types.assert_called_once_with(
        from_relation=temp,
        to_relation=target,
    )
    adapter.plan_incremental_arguments.assert_called_once_with(
        target_relation=target,
        temp_relation=temp,
        unique_key="id",
        dest_columns=["id"],
        incremental_predicates=["id > 0"],
        adapter_arguments={
            "catalog_relation": state.catalog_relation,
            "incremental_plan": state.incremental_plan,
        },
    )
    strategy_macro.assert_called_once_with({"target_relation": target})


def test_incremental_executor_runs_python_lifecycle_and_mutation(monkeypatch):
    existing = _relation("existing")
    context, target, intermediate, backup, temp = _context(existing)
    state = IncrementalMaterializationExecutionState(
        existing_relation=existing,
        target_relation=target,
        intermediate_relation=intermediate,
        backup_relation=backup,
        temp_relation=temp,
        preexisting_intermediate_relation=None,
        preexisting_backup_relation=None,
        incremental_plan=SimpleNamespace(),
        strategy_macro=MagicMock(),
        catalog_relation=None,
        unique_key="id",
        staging_is_temporary=True,
        full_refresh_mode=False,
        on_schema_change="ignore",
        grant_config={"select": ["reporter"]},
    )
    adapter = MagicMock()
    executor = IncrementalMaterializationExecutor(adapter, _model(), context)
    monkeypatch.setattr(
        executor,
        "resolve_incremental_execution_state",
        MagicMock(return_value=state),
    )
    monkeypatch.setattr(
        executor,
        "_incremental_mutation_sql",
        MagicMock(return_value="merge into target"),
    )
    monkeypatch.setattr(executor, "_execute_main", MagicMock())

    result = executor.execute()

    assert result == {"relations": [target]}
    executor._incremental_mutation_sql.assert_called_once_with(state)
    executor._execute_main.assert_called_once_with("merge into target")
    context["create_indexes"].assert_not_called()
    context["run_hooks"].assert_has_calls(
        [
            call(context["pre_hooks"], inside_transaction=False),
            call(context["pre_hooks"], inside_transaction=True),
            call(context["post_hooks"], inside_transaction=True),
            call(context["post_hooks"], inside_transaction=False),
        ]
    )
    context["apply_grants"].assert_called_once_with(
        target,
        {"select": ["reporter"]},
        should_revoke=False,
    )
    context["adapter"].commit.assert_called_once_with()
    adapter.rename_relation.assert_not_called()
    adapter.drop_relation.assert_not_called()


def test_incremental_executor_full_refresh_swaps_and_drops_backup(monkeypatch):
    existing = _relation("existing")
    context, target, intermediate, backup, temp = _context(existing)
    state = IncrementalMaterializationExecutionState(
        existing_relation=existing,
        target_relation=target,
        intermediate_relation=intermediate,
        backup_relation=backup,
        temp_relation=temp,
        preexisting_intermediate_relation=None,
        preexisting_backup_relation=None,
        incremental_plan=SimpleNamespace(),
        strategy_macro=MagicMock(),
        catalog_relation=None,
        unique_key="id",
        staging_is_temporary=True,
        full_refresh_mode=True,
        on_schema_change="ignore",
        grant_config={},
    )
    adapter = MagicMock()
    executor = IncrementalMaterializationExecutor(adapter, _model(), context)
    monkeypatch.setattr(
        executor,
        "resolve_incremental_execution_state",
        MagicMock(return_value=state),
    )
    monkeypatch.setattr(executor, "_build_sql", MagicMock(return_value="create intermediate"))
    monkeypatch.setattr(executor, "_execute_main", MagicMock())

    executor.execute()

    executor._build_sql.assert_called_once_with(intermediate)
    context["create_indexes"].assert_called_once_with(intermediate)
    assert adapter.rename_relation.call_args_list == [
        call(target, backup),
        call(intermediate, target),
    ]
    adapter.drop_relation.assert_called_once_with(backup)
