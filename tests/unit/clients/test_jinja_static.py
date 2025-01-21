import pytest

from dbt.artifacts.resources import RefArgs
from dbt.clients.jinja_static import (
    statically_extract_has_name_this,
    statically_extract_macro_calls,
    statically_parse_ref_or_source,
    statically_parse_unrendered_config,
)
from dbt.context.base import generate_base_context
from dbt.exceptions import ParsingError
from dbt_common.clients.jinja import MacroType
from dbt_common.clients.jinja_macro_call import DbtMacroCall


@pytest.mark.parametrize(
    "macro_string,expected_possible_macro_calls",
    [
        (
            "{% macro parent_macro() %} {% do return(nested_macro()) %} {% endmacro %}",
            ["nested_macro"],
        ),
        (
            "{% macro lr_macro() %} {{ return(load_result('relations').table) }} {% endmacro %}",
            ["load_result"],
        ),
        (
            "{% macro get_snapshot_unique_id() -%} {{ return(adapter.dispatch('get_snapshot_unique_id')()) }} {%- endmacro %}",
            ["get_snapshot_unique_id"],
        ),
        (
            "{% macro get_columns_in_query(select_sql) -%} {{ return(adapter.dispatch('get_columns_in_query')(select_sql)) }} {% endmacro %}",
            ["get_columns_in_query"],
        ),
        (
            """{% macro test_mutually_exclusive_ranges(model) %}
            with base as (
                select {{ get_snapshot_unique_id() }} as dbt_unique_id,
                *
                from {{ model }} )
            {% endmacro %}""",
            ["get_snapshot_unique_id"],
        ),
        (
            "{% macro test_my_test(model) %} select {{ current_timestamp_backcompat() }} {% endmacro %}",
            ["current_timestamp_backcompat"],
        ),
        (
            "{% macro some_test(model) -%} {{ return(adapter.dispatch('test_some_kind4', 'foo_utils4')) }} {%- endmacro %}",
            ["test_some_kind4", "foo_utils4.test_some_kind4"],
        ),
        (
            "{% macro some_test(model) -%} {{ return(adapter.dispatch('test_some_kind5', macro_namespace = 'foo_utils5')) }} {%- endmacro %}",
            ["test_some_kind5", "foo_utils5.test_some_kind5"],
        ),
    ],
)
def test_extract_macro_calls(macro_string, expected_possible_macro_calls):
    cli_vars = {"local_utils_dispatch_list": ["foo_utils4"]}
    ctx = generate_base_context(cli_vars)

    possible_macro_calls = statically_extract_macro_calls(macro_string, ctx)
    assert [c.name for c in possible_macro_calls] == expected_possible_macro_calls


@pytest.mark.parametrize(
    "macro_string,expected_possible_macro_calls",
    [
        (
            "{% macro parent_macro() %} {% do nested_macro(12, 'string', 1.0, True, my_kwarg = 2) %} {% endmacro %}",
            [
                DbtMacroCall(
                    name="nested_macro",
                    source="",
                    arg_types=[
                        MacroType(name="int"),
                        MacroType(name="str"),
                        MacroType(name="float"),
                        MacroType(name="bool"),
                    ],
                    kwarg_types={
                        "my_kwarg": MacroType(name="int"),
                    },
                ),
            ],
        )
    ],
)
def test_extract_macro_calls_with_type_info(macro_string, expected_possible_macro_calls):
    possible_macro_calls = statically_extract_macro_calls(macro_string, {})
    assert possible_macro_calls == expected_possible_macro_calls


class TestStaticallyParseRefOrSource:
    def test_invalid_expression(self):
        with pytest.raises(ParsingError):
            statically_parse_ref_or_source("invalid")

    @pytest.mark.parametrize(
        "expression,expected_ref_or_source",
        [
            ("ref('model')", RefArgs(name="model")),
            ("ref('package','model')", RefArgs(name="model", package="package")),
            ("ref('model',v=3)", RefArgs(name="model", version=3)),
            ("ref('package','model',v=3)", RefArgs(name="model", package="package", version=3)),
            ("source('schema', 'table')", ["schema", "table"]),
        ],
    )
    def test_valid_ref_expression(self, expression, expected_ref_or_source):
        ref_or_source = statically_parse_ref_or_source(expression)
        assert ref_or_source == expected_ref_or_source


class TestStaticallyParseUnrenderedConfig:
    @pytest.mark.parametrize(
        "expression,expected_unrendered_config",
        [
            (
                "{{ config(materialized='view') }}",
                {"materialized": "Keyword(key='materialized', value=Const(value='view'))"},
            ),
            (
                "{{ config(materialized='view', enabled=True) }}",
                {
                    "materialized": "Keyword(key='materialized', value=Const(value='view'))",
                    "enabled": "Keyword(key='enabled', value=Const(value=True))",
                },
            ),
            (
                "{{ config(materialized=env_var('test')) }}",
                {
                    "materialized": "Keyword(key='materialized', value=Call(node=Name(name='env_var', ctx='load'), args=[Const(value='test')], kwargs=[], dyn_args=None, dyn_kwargs=None))"
                },
            ),
            (
                "{{ config(materialized=env_var('test', default='default')) }}",
                {
                    "materialized": "Keyword(key='materialized', value=Call(node=Name(name='env_var', ctx='load'), args=[Const(value='test')], kwargs=[Keyword(key='default', value=Const(value='default'))], dyn_args=None, dyn_kwargs=None))"
                },
            ),
            (
                "{{ config(materialized=env_var('test', default=env_var('default'))) }}",
                {
                    "materialized": "Keyword(key='materialized', value=Call(node=Name(name='env_var', ctx='load'), args=[Const(value='test')], kwargs=[Keyword(key='default', value=Call(node=Name(name='env_var', ctx='load'), args=[Const(value='default')], kwargs=[], dyn_args=None, dyn_kwargs=None))], dyn_args=None, dyn_kwargs=None))"
                },
            ),
        ],
    )
    def test_statically_parse_unrendered_config(self, expression, expected_unrendered_config):
        unrendered_config = statically_parse_unrendered_config(expression)
        assert unrendered_config == expected_unrendered_config


@pytest.mark.parametrize(
    "raw_code,expected_result",
    [
        ("{{ this }}", True),
        ("{{ this.variable }}", True),
        ("{{ log(this) }}", True),
        ("{{ some_other_this }}", False),
        ("this", False),
        ("{{ some_object.this }}", False),
    ],
)
def test_statically_extract_has_name_this(raw_code: str, expected_result: bool) -> None:
    assert statically_extract_has_name_this(raw_code) == expected_result
