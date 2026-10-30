from dataclasses import replace
from unittest import mock

import pytest

from dbt.artifacts.resources import ColumnInfo, Contract
from dbt.contracts.graph.nodes import ModelNode
from dbt.exceptions import ContractBreakingChangeError
from tests.unit.fixtures import model_node


@pytest.mark.parametrize(
    "input_type,expected_normalized",
    [
        ("varchar(10)", "varchar"),
        ("VARCHAR(5)", "varchar"),
        ("numeric(10,2)", "numeric"),
        ("numeric(10, 2)", "numeric"),
        ("decimal(5)", "decimal"),
        ("text", "text"),
        ("STRUCT(a INTEGER)", "struct(a integer)"),
        ("STRUCT(a VARCHAR)", "struct(a varchar)"),
        ("STRUCT(a VARCHAR(10), b INT)", "struct(a varchar, b int)"),
        ("Nullable(UUID)", "nullable(uuid)"),
        ("Nullable(Int64)", "nullable(int64)"),
        ("Nullable(Decimal(10, 2))", "nullable(decimal)"),
        ("timestamp(6) with time zone", "timestamp with time zone"),
        ("timestamp(3) with time zone", "timestamp with time zone"),
        ("timestamp with time zone", "timestamp with time zone"),
        ("Array(Tuple(String, Int32))", "array(tuple(string, int32))"),
        (None, None),
        ("", ""),
    ],
)
def test_normalize_data_type_for_comparison(input_type, expected_normalized):
    assert ModelNode._normalize_data_type_for_comparison(input_type) == expected_normalized


def test_same_contract_parameterized_data_types():
    base = model_node()
    base = replace(
        base,
        version="1",
        contract=Contract(enforced=True, checksum="old_hash"),
        columns={"payload": ColumnInfo(name="payload", data_type="STRUCT(a INTEGER)")},
    )

    # Inner type change in STRUCT is a breaking change
    struct_changed = replace(
        base,
        contract=Contract(enforced=True, checksum="new_hash_1"),
        columns={"payload": ColumnInfo(name="payload", data_type="STRUCT(a VARCHAR)")},
    )
    with mock.patch("dbt.contracts.graph.nodes.get_adapter_constraint_support", return_value={}):
        with pytest.raises(ContractBreakingChangeError) as exc_info:
            struct_changed.same_contract(base)
        assert "STRUCT(a INTEGER) -> STRUCT(a VARCHAR)" in str(exc_info.value)

    # Inner type change in Nullable is a breaking change
    nullable_old = replace(
        base,
        contract=Contract(enforced=True, checksum="old_hash_nullable"),
        columns={"id": ColumnInfo(name="id", data_type="Nullable(UUID)")},
    )
    nullable_new = replace(
        base,
        contract=Contract(enforced=True, checksum="new_hash_nullable"),
        columns={"id": ColumnInfo(name="id", data_type="Nullable(Int64)")},
    )
    with mock.patch("dbt.contracts.graph.nodes.get_adapter_constraint_support", return_value={}):
        with pytest.raises(ContractBreakingChangeError) as exc_info:
            nullable_new.same_contract(nullable_old)
        assert "Nullable(UUID) -> Nullable(Int64)" in str(exc_info.value)

    # Internal precision change in Nullable(Decimal(10, 2)) -> Nullable(Decimal(12, 4)) is not breaking
    dec_old = replace(
        base,
        contract=Contract(enforced=True, checksum="old_hash_dec"),
        columns={"amount": ColumnInfo(name="amount", data_type="Nullable(Decimal(10, 2))")},
    )
    dec_new = replace(
        base,
        contract=Contract(enforced=True, checksum="new_hash_dec"),
        columns={"amount": ColumnInfo(name="amount", data_type="Nullable(Decimal(12, 4))")},
    )
    with mock.patch("dbt.contracts.graph.nodes.get_adapter_constraint_support", return_value={}):
        assert dec_new.same_contract(dec_old) is False

    # Internal precision change in STRUCT field is not breaking
    struct_prec_old = replace(
        base,
        contract=Contract(enforced=True, checksum="old_hash_sp"),
        columns={"data": ColumnInfo(name="data", data_type="STRUCT(a VARCHAR(10), b INT)")},
    )
    struct_prec_new = replace(
        base,
        contract=Contract(enforced=True, checksum="new_hash_sp"),
        columns={"data": ColumnInfo(name="data", data_type="STRUCT(a VARCHAR(20), b INT)")},
    )
    with mock.patch("dbt.contracts.graph.nodes.get_adapter_constraint_support", return_value={}):
        assert struct_prec_new.same_contract(struct_prec_old) is False
