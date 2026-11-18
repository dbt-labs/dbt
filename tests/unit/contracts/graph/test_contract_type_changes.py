from dataclasses import replace

import pytest

from dbt.artifacts.resources import ColumnInfo, Contract
from dbt.contracts.graph.nodes import ModelNode
from dbt.exceptions import ContractBreakingChangeError
from tests.unit.fixtures import model_node


@pytest.fixture
def no_adapter_constraint_support(monkeypatch):
    # same_contract looks up the adapter's constraint support, which needs a loaded adapter
    monkeypatch.setattr(
        "dbt.contracts.graph.nodes.get_adapter_constraint_support", lambda adapter_type: {}
    )


def _contracted_versioned_model(data_type: str, checksum: str) -> ModelNode:
    return replace(
        model_node(),
        version=1,
        contract=Contract(enforced=True, checksum=checksum),
        columns={"payload": ColumnInfo(name="payload", data_type=data_type)},
    )


@pytest.mark.parametrize(
    "old_data_type,new_data_type",
    [
        ("varchar(3)", "varchar(10)"),
        ("VARCHAR(5)", "varchar(20)"),
        ("numeric(10,2)", "numeric(12,4)"),
        ("numeric(10, 2)", "numeric(12,4)"),
        ("varchar(max)", "varchar(256)"),
        ("timestamp(6) with time zone", "timestamp(3) with time zone"),
        ("Nullable(Decimal(10, 2))", "Nullable(Decimal(12, 4))"),
        ("Array(Nullable(String(10)))", "Array(Nullable(String(20)))"),
        ("STRUCT(a INTEGER, b VARCHAR(10))", "struct(a integer,  b varchar(20))"),
    ],
)
@pytest.mark.usefixtures("no_adapter_constraint_support")
def test_same_contract_ignores_size_precision_and_scale(old_data_type, new_data_type):
    old = _contracted_versioned_model(old_data_type, checksum="old")
    new = _contracted_versioned_model(new_data_type, checksum="new")
    # a change, but not a breaking one, so no ContractBreakingChangeError
    assert new.same_contract(old, "postgres") is False


@pytest.mark.parametrize(
    "old_data_type,new_data_type",
    [
        ("integer", "varchar"),
        ("STRUCT(a INTEGER)", "STRUCT(a VARCHAR)"),
        ("Nullable(UUID)", "Nullable(Int64)"),
        ("LowCardinality(Nullable(String))", "LowCardinality(Nullable(Int64))"),
        ("Array(String)", "Array(Int64)"),
        ("Nullable(Decimal(10, 2))", "Nullable(Float64)"),
        ("timestamp(6) with time zone", "timestamp"),
    ],
)
@pytest.mark.usefixtures("no_adapter_constraint_support")
def test_same_contract_detects_type_argument_changes(old_data_type, new_data_type):
    old = _contracted_versioned_model(old_data_type, checksum="old")
    new = _contracted_versioned_model(new_data_type, checksum="new")
    with pytest.raises(ContractBreakingChangeError) as excinfo:
        new.same_contract(old, "postgres")
    assert f"payload ({old_data_type} -> {new_data_type})" in str(excinfo.value)
