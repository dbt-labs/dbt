from types import SimpleNamespace
from unittest.mock import MagicMock, call

import pytest

from dbt.exceptions import DbtInternalError
from dbt.materializations.table import TableMaterializationExecutor


def _relation(name, relation_type="table"):
    relation = MagicMock(name=name)
    relation.name = name
    relation.type = relation_type
    relation.include.return_value = f'"analytics"."{name}"'
    return relation


def _executor_context(*, existing_relation=None, grant_config=None):
    this = MagicMock(name="this")
    target = _relation("target")
    intermediate = _relation("intermediate")
    backup = _relation("backup")
    this.incorporate.return_value = target

    load_cached_relation = MagicMock(
        side_effect=[existing_relation, None, None, existing_relation]
    )
    context = {
        "adapter": MagicMock(),
        "this": this,
        "config": {"grants": grant_config},
        "model": {"unique_id": "model.test.orders"},
        "sql": "select 1 as id",
        "pre_hooks": ["pre"],
        "post_hooks": ["post"],
        "load_cached_relation": load_cached_relation,
        "make_intermediate_relation": MagicMock(return_value=intermediate),
        "make_backup_relation": MagicMock(return_value=backup),
        "drop_relation_if_exists": MagicMock(),
        "run_hooks": MagicMock(),
        "get_create_table_as_sql": MagicMock(return_value="create table intermediate as select 1"),
        "write": MagicMock(),
        "store_result": MagicMock(),
        "create_indexes": MagicMock(),
        "should_revoke": MagicMock(return_value=True),
        "apply_grants": MagicMock(),
        "persist_docs": MagicMock(),
    }
    return context, target, intermediate, backup


def test_table_executor_runs_builtin_lifecycle_in_python():
    existing = _relation("existing", "view")
    context, target, intermediate, backup = _executor_context(
        existing_relation=existing,
        grant_config={"select": ["reporter"]},
    )
    response = object()
    table = object()
    adapter = MagicMock(spec=["execute", "rename_relation"])
    adapter.execute.return_value = (response, table)

    result = TableMaterializationExecutor(adapter, MagicMock(), context).execute()

    assert result == {"relations": [target]}
    context["load_cached_relation"].assert_has_calls(
        [call(context["this"]), call(intermediate), call(backup), call(existing)]
    )
    context["run_hooks"].assert_has_calls(
        [
            call(context["pre_hooks"], inside_transaction=False),
            call(context["pre_hooks"], inside_transaction=True),
            call(context["post_hooks"], inside_transaction=True),
            call(context["post_hooks"], inside_transaction=False),
        ]
    )
    adapter.execute.assert_called_once_with(
        "create table intermediate as select 1", auto_begin=True, fetch=False
    )
    context["store_result"].assert_called_once_with("main", response=response, agate_table=table)
    assert adapter.rename_relation.call_args_list == [
        call(existing, backup),
        call(intermediate, target),
    ]
    context["apply_grants"].assert_called_once_with(
        target,
        {"select": ["reporter"]},
        should_revoke=True,
    )
    context["persist_docs"].assert_called_once_with(target, context["model"])
    context["adapter"].commit.assert_called_once_with()
    assert context["drop_relation_if_exists"].call_args_list == [
        call(None),
        call(None),
        call(backup),
    ]


def test_table_executor_rejects_missing_or_untyped_context_boundaries():
    executor = TableMaterializationExecutor(MagicMock(), MagicMock(), {})

    with pytest.raises(DbtInternalError, match="missing 'this'"):
        executor.resolve_execution_state()

    executor.context["this"] = object()
    executor.context["load_cached_relation"] = "not callable"
    with pytest.raises(DbtInternalError, match="not callable"):
        executor.resolve_execution_state()


class _RenderArguments:
    def __init__(self, **kwargs):
        self.__dict__.update(kwargs)


def _macro(unique_id):
    return SimpleNamespace(macro=SimpleNamespace(unique_id=unique_id))


def _typed_renderer_context():
    context, _, intermediate, _ = _executor_context()
    context["get_create_table_as_sql"].macro.unique_id = "macro.dbt.get_create_table_as_sql"
    context["render_create_from_query_plan"] = _macro("macro.dbt.render_create_from_query_plan")
    adapter_wrapper = MagicMock()
    adapter_wrapper.dispatch.side_effect = (
        _macro("macro.dbt.default__get_create_table_as_sql"),
        _macro("macro.dbt.default__render_create_from_query_plan"),
        _macro("macro.dbt.default__create_table_as"),
    )
    context["adapter"] = adapter_wrapper
    context["render_create_from_query_ctas"] = _macro("macro.dbt.render_create_from_query_ctas")
    context["config"] = {
        "grants": None,
        "contract": {"enforced": False},
        "sql_header": "alter session set query_tag = 'dbt'",
    }
    return context, intermediate


def test_table_executor_uses_typed_python_renderer(monkeypatch):
    context, intermediate = _typed_renderer_context()
    create_plan = SimpleNamespace(renderer_macro="render_create_from_query_ctas")
    render_result = SimpleNamespace(
        kind=SimpleNamespace(value="sql"),
        sql="create table typed as select 1",
    )
    adapter = MagicMock()
    adapter.plan_create_from_query.return_value = create_plan
    adapter.resolve_create_from_query_render.return_value = render_result
    executor = TableMaterializationExecutor(adapter, MagicMock(), context)
    monkeypatch.setattr(executor, "_render_arguments_type", lambda: _RenderArguments)

    sql = executor._build_sql(intermediate)

    assert sql == "create table typed as select 1"
    context["get_create_table_as_sql"].assert_not_called()
    adapter.plan_create_from_query.assert_called_once_with(False, intermediate, executor.model)
    arguments = adapter.resolve_create_from_query_render.call_args.args[1]
    assert arguments.relation_sql == '"analytics"."intermediate"'
    assert arguments.query == context["sql"]
    assert arguments.sql_header == "alter session set query_tag = 'dbt'"
    assert arguments.contract_enforced is False
    assert arguments.legacy_renderer_override is None


def test_table_executor_honors_typed_legacy_fallback(monkeypatch):
    context, intermediate = _typed_renderer_context()
    create_plan = SimpleNamespace(renderer_macro="render_create_from_query_ctas")
    adapter = MagicMock()
    adapter.plan_create_from_query.return_value = create_plan
    adapter.resolve_create_from_query_render.return_value = SimpleNamespace(
        kind=SimpleNamespace(value="legacy_macro"),
        renderer_macro="get_create_table_as_sql",
        reason="Enforced table contracts still require the compatibility renderer",
    )
    executor = TableMaterializationExecutor(adapter, MagicMock(), context)
    monkeypatch.setattr(executor, "_render_arguments_type", lambda: _RenderArguments)

    sql = executor._build_sql(intermediate)

    assert sql == "create table intermediate as select 1"
    context["get_create_table_as_sql"].assert_called_once_with(False, intermediate, context["sql"])


def test_table_executor_reports_project_renderer_override(monkeypatch):
    context, intermediate = _typed_renderer_context()
    context["get_create_table_as_sql"].macro.unique_id = "macro.project.get_create_table_as_sql"
    create_plan = SimpleNamespace(renderer_macro="render_create_from_query_ctas")
    adapter = MagicMock()
    adapter.plan_create_from_query.return_value = create_plan
    adapter.resolve_create_from_query_render.return_value = SimpleNamespace(
        kind=SimpleNamespace(value="legacy_macro"),
        renderer_macro="get_create_table_as_sql",
    )
    executor = TableMaterializationExecutor(adapter, MagicMock(), context)
    monkeypatch.setattr(executor, "_render_arguments_type", lambda: _RenderArguments)

    executor._build_sql(intermediate)

    arguments = adapter.resolve_create_from_query_render.call_args.args[1]
    assert arguments.legacy_renderer_override == "macro.project.get_create_table_as_sql"


def test_table_executor_reports_adapter_dispatch_override():
    context, _ = _typed_renderer_context()
    context["adapter"].dispatch.side_effect = (
        _macro("macro.adapter.adapter__get_create_table_as_sql"),
    )
    executor = TableMaterializationExecutor(MagicMock(), MagicMock(), context)

    override = executor._legacy_renderer_override(
        SimpleNamespace(renderer_macro="render_create_from_query_ctas")
    )

    assert override == "macro.adapter.adapter__get_create_table_as_sql"
