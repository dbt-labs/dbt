-- funcsign: () -> string
{%- macro current_timestamp() -%}
    {{ adapter.dispatch('current_timestamp', 'dbt')() }}
{%- endmacro -%}

-- funcsign: () -> string
{% macro default__current_timestamp() -%}
  {{ exceptions.raise_not_implemented(
    'current_timestamp macro not implemented for adapter ' + adapter.type()) }}
{%- endmacro %}

-- funcsign: () -> string
{%- macro snapshot_get_time() -%}
    {{ adapter.dispatch('snapshot_get_time', 'dbt')() }}
{%- endmacro -%}

-- funcsign: () -> string
{% macro default__snapshot_get_time() %}
    {{ current_timestamp() }}
{% endmacro %}

-- funcsign: () -> optional[string]
{% macro get_snapshot_get_time_data_type() %}
    {% set snapshot_time = adapter.dispatch('snapshot_get_time', 'dbt')() %}
    {% set time_data_type_sql = 'select ' ~ snapshot_time ~ ' as dbt_snapshot_time' %}
    {% set snapshot_time_column_schema = get_column_schema_from_query(time_data_type_sql) %}
    {% set time_data_type = snapshot_time_column_schema[0].dtype %}
    {{ return(time_data_type or none) }}
{% endmacro %}

---------------------------------------------

/* {#
    DEPRECATED: DO NOT USE IN NEW PROJECTS

    This is ONLY to handle the fact that Snowflake + Postgres had functionally
    different implementations of {{ dbt.current_timestamp }} + {{ dbt_utils.current_timestamp }}

    If you had a project or package that called {{ dbt_utils.current_timestamp() }}, you should
    continue to use this macro to guarantee identical behavior on those two databases.
#} */

-- funcsign: () -> string
{% macro current_timestamp_backcompat() %}
    {{ return(adapter.dispatch('current_timestamp_backcompat', 'dbt')()) }}
{% endmacro %}

-- funcsign: () -> string
{% macro default__current_timestamp_backcompat() %}
    current_timestamp::timestamp
{% endmacro %}

-- funcsign: () -> string
{% macro current_timestamp_in_utc_backcompat() %}
    {{ return(adapter.dispatch('current_timestamp_in_utc_backcompat', 'dbt')()) }}
{% endmacro %}

-- funcsign: () -> string
{% macro default__current_timestamp_in_utc_backcompat() %}
    {{ return(adapter.dispatch('current_timestamp_backcompat', 'dbt')()) }}
{% endmacro %}
