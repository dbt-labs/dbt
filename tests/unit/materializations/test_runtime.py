from types import SimpleNamespace
from unittest.mock import MagicMock

from dbt.adapters.base.relation import BaseRelation
from dbt.adapters.planning import (
    CatalogBindingState,
    CatalogFacts,
    CreateFromQueryFacts,
    ExistingRelationFacts,
    FormatFacts,
    MergeExistingSnapshot,
    PlanProvenance,
    RelationFacts,
    RuntimeFacts,
    SnapshotHardDeletes,
    SnapshotMaterializationPlan,
    SnapshotStrategyFacts,
    TableMaterializationFacts,
)
from dbt.materializations.runtime import MaterializationRuntime


def _table_facts() -> TableMaterializationFacts:
    relation = RelationFacts("database", "schema", "accounts", "table")
    return TableMaterializationFacts(
        create=CreateFromQueryFacts(
            relation=relation,
            catalog=CatalogFacts(state=CatalogBindingState.UNBOUND),
            format=FormatFacts(),
            runtime=RuntimeFacts(engine="test"),
        ),
        existing=ExistingRelationFacts(
            relation=relation,
            format=FormatFacts(),
            can_be_renamed=True,
            can_be_replaced=True,
            requires_drop_before_replace=False,
        ),
    )


def test_snapshot_runtime_normalizes_strategy_and_relations() -> None:
    target = MagicMock(spec=BaseRelation)
    target.is_table = True
    staging = MagicMock(spec=BaseRelation)
    strategy_macro = MagicMock(
        return_value={
            "unique_key": ["account_id", "line_id"],
            "updated_at": "source_data.updated_at",
            "row_changed": "snapshotted_data.updated_at < source_data.updated_at",
            "scd_id": "md5(source_data.account_id)",
            "hard_deletes": "new_record",
        }
    )
    config = MagicMock()
    config.get.side_effect = lambda name, default=None: {
        "strategy": "timestamp",
    }.get(name, default)
    adapter = MagicMock()
    adapter.build_table_materialization_facts.return_value = _table_facts()
    model = SimpleNamespace(
        alias="accounts",
        name="accounts",
        database="database",
        schema="schema",
    )
    context_model = {"config": {}, "compiled_code": "select * from source"}
    runtime = MaterializationRuntime(
        adapter=adapter,
        model=model,
        context={
            "config": config,
            "model": context_model,
            "get_or_create_relation": MagicMock(return_value=[True, target]),
            "make_temp_relation": MagicMock(return_value=staging),
            "strategy_dispatch": MagicMock(return_value=strategy_macro),
        },
    )
    plan = SnapshotMaterializationPlan(
        materialization_macro_id="macro.dbt.materialization_snapshot_default",
        provenance=(PlanProvenance("test.snapshot", "typed snapshot"),),
    )

    resolved = runtime.resolve_snapshot_strategy(plan)

    assert isinstance(resolved, MergeExistingSnapshot)
    assert resolved.facts.strategy.unique_key == ("account_id", "line_id")
    assert resolved.facts.strategy.hard_deletes.value == "new_record"
    assert runtime.snapshot_relations().target is target
    assert runtime.snapshot_relations().staging is staging
    strategy_macro.assert_called_once_with(
        context_model,
        "snapshotted_data",
        "source_data",
        {},
        True,
    )


def test_snapshot_schema_reconciliation_removes_control_columns() -> None:
    staging = MagicMock(spec=BaseRelation)
    target = MagicMock(spec=BaseRelation)
    config = MagicMock()
    create_columns = MagicMock()
    adapter = MagicMock()
    adapter.get_missing_columns.return_value = (
        SimpleNamespace(name="payload"),
        SimpleNamespace(name="dbt_change_type"),
        SimpleNamespace(name="dbt_unique_key_2"),
    )
    adapter.get_columns_in_relation.return_value = (
        SimpleNamespace(name="account_id"),
        SimpleNamespace(name="dbt_unique_key_1"),
        SimpleNamespace(name="payload"),
    )
    adapter.quote.side_effect = lambda name: f'"{name}"'
    runtime = MaterializationRuntime(
        adapter=adapter,
        model=MagicMock(),
        context={"config": config, "create_columns": create_columns},
    )
    strategy_facts = SnapshotStrategyFacts(
        unique_key=("account_id", "line_id"),
        updated_at="source_data.updated_at",
        row_changed="true",
        scd_id="md5(source_data.account_id)",
        hard_deletes=SnapshotHardDeletes.IGNORE,
    )

    columns = runtime.reconcile_snapshot_columns(
        staging,
        target,
        strategy_facts,
    )

    assert columns == ('"account_id"', '"payload"')
    create_columns.assert_called_once()
    assert tuple(create_columns.call_args.args[1]) == (
        adapter.get_missing_columns.return_value[0],
    )


def test_builtin_snapshot_strategy_and_staging_builder_use_native_lifecycle() -> None:
    config = MagicMock()
    config.get.side_effect = lambda name, default=None: {
        "strategy": "timestamp",
    }.get(name, default)
    helper = MagicMock()
    helper.macro.unique_id = "macro.dbt.build_snapshot_staging_table"
    runtime = MaterializationRuntime(
        adapter=MagicMock(),
        model=MagicMock(),
        context={
            "config": config,
            "build_snapshot_staging_table": helper,
        },
    )

    assert runtime.supports_snapshot_materialization() is True
