from types import SimpleNamespace
from unittest.mock import MagicMock, call

import pytest

from dbt.exceptions import DbtInternalError
from dbt.materializations.table import TableMaterializationExecutor


def _relation(name, relation_type="table"):
    return SimpleNamespace(name=name, type=relation_type)


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
    adapter = MagicMock()
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
        executor.plan()

    executor.context["this"] = object()
    executor.context["load_cached_relation"] = "not callable"
    with pytest.raises(DbtInternalError, match="not callable"):
        executor.plan()
