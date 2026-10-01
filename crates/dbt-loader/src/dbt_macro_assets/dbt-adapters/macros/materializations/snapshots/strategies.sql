{#
    Dispatch strategies by name, optionally qualified to a package
#}
-- funcsign: (string) -> (model, string, string, model.config, bool) -> strategy
{% macro strategy_dispatch(name) -%}
{% set original_name = name %}
  {% if '.' in name %}
    {% set package_name, name = name.split(".", 1) %}
  {% else %}
    {% set package_name = none %}
  {% endif %}

  {% if package_name is none %}
    {% set package_context = context %}
  {% elif package_name in context %}
    {% set package_context = context[package_name] %}
  {% else %}
    {% set error_msg %}
        Could not find package '{{package_name}}', called with '{{original_name}}'
    {% endset %}
    {{ exceptions.raise_compiler_error(error_msg | trim) }}
  {% endif %}

  {%- set search_name = 'snapshot_' ~ name ~ '_strategy' -%}

  {% if search_name not in package_context %}
    {% set error_msg %}
        The specified strategy macro '{{name}}' was not found in package '{{ package_name }}'
    {% endset %}
    {{ exceptions.raise_compiler_error(error_msg | trim) }}
  {% endif %}
  {{ return(package_context[search_name]) }}
{%- endmacro %}


{#
    Create SCD Hash SQL fields cross-db
#}
-- funcsign: (list[string]) -> string
{% macro snapshot_hash_arguments(args) -%}
  {{ adapter.dispatch('snapshot_hash_arguments', 'dbt')(args) }}
{%- endmacro %}

-- funcsign: (list[string]) -> string
{% macro default__snapshot_hash_arguments(args) -%}
    md5({%- for arg in args -%}
        coalesce(cast({{ arg }} as varchar ), '')
        {% if not loop.last %} || '|' || {% endif %}
    {%- endfor -%})
{%- endmacro %}

{#
    Core strategy definitions
#}

{% macro snapshot_timestamp_strategy(node, snapshotted_rel, current_rel, model_config, target_exists) %}
    {# The model_config parameter is no longer used, but is passed in anyway for compatibility. #}
    {% set primary_key = config.get('unique_key') %}
    {% set updated_at = config.get('updated_at') %}
    {% set hard_deletes = adapter.get_hard_deletes_behavior(config) %}
    {% set invalidate_hard_deletes = hard_deletes == 'invalidate' %}
    {% set columns = config.get("snapshot_table_column_names") or get_snapshot_table_column_names() %}

    {#/*
        The snapshot relation might not have an {{ updated_at }} value if the
        snapshot strategy is changed from `check` to `timestamp`. We
        should use a dbt-created column for the comparison in the snapshot
        table instead of assuming that the user-supplied {{ updated_at }}
        will be present in the historical data.

        See https://github.com/dbt-labs/dbt-core/issues/2350
    */ #}
    {% set row_changed_expr -%}
        ({{ snapshotted_rel }}.{{ columns.dbt_valid_from }} < {{ current_rel }}.{{ updated_at }})
    {%- endset %}

    {% set scd_args = api.Relation.scd_args(primary_key, updated_at) %} -- noqa: updated_at should be a string
    {% set scd_id_expr = snapshot_hash_arguments(scd_args) %}

    {% do return({
        "unique_key": primary_key,
        "updated_at": updated_at,
        "row_changed": row_changed_expr,
        "scd_id": scd_id_expr,
        "invalidate_hard_deletes": invalidate_hard_deletes,
        "hard_deletes": hard_deletes
    }) %}
{% endmacro %}


-- funcsign: (timestamp) -> string
{% macro snapshot_string_as_time(timestamp) -%}
    {{ adapter.dispatch('snapshot_string_as_time', 'dbt')(timestamp) }}
{%- endmacro %}

-- funcsign: (timestamp) -> string
{% macro default__snapshot_string_as_time(timestamp) %}
    {% do exceptions.raise_not_implemented(
        'snapshot_string_as_time macro not implemented for adapter '+adapter.type()
    ) %}
{% endmacro %}


-- funcsign: (model, bool, list[string]|string) -> tuple[bool, list[base_column]]
{% macro snapshot_check_all_get_existing_columns(node, target_exists, check_cols_config) -%}
    {%- if not target_exists -%}
        {#-- no table yet -> return whatever the query does --#}
        {{ return((false, query_columns)) }}
    {%- endif -%}

    {#-- handle any schema changes --#}
    {%- set target_relation = adapter.get_relation(database=node.database, schema=node.schema, identifier=node.alias) -%}

    {% if check_cols_config == 'all' %}
        {%- set query_columns = get_columns_in_query(node['compiled_code']) -%}

    {% elif check_cols_config is iterable and (check_cols_config | length) > 0 %}
        {#-- query for proper casing/quoting, to support comparison below --#}
        {%- set select_check_cols_from_target -%}
            {#-- N.B. The whitespace below is necessary to avoid edge case issue with comments --#}
            {#-- See: https://github.com/dbt-labs/dbt-core/issues/6781 --#}
            select {{ check_cols_config | join(', ') }} from (
                {{ node['compiled_code'] }}
            ) subq
        {%- endset -%}
        {% set query_columns = get_columns_in_query(select_check_cols_from_target) %}

    {% else %}
        {% do exceptions.raise_compiler_error("Invalid value for 'check_cols': " ~ check_cols_config) %}
    {% endif %}

    {%- set existing_cols = adapter.get_columns_in_relation(target_relation) | map(attribute = 'name') | list -%}
    {%- set ns = namespace() -%} {#-- handle for-loop scoping with a namespace --#}
    {%- set ns.column_added = false -%}

    {%- set intersection = [] -%}
    {%- for col in query_columns -%}
        {%- if adapter.dispatch('snapshot_check_column_exists', 'dbt')(col, existing_cols) -%}
            {%- do intersection.append(adapter.quote(col)) -%}
        {%- else -%}
            {% set ns.column_added = true %}
        {%- endif -%}
    {%- endfor -%}
    {{ return((ns.column_added, intersection)) }}
{%- endmacro %}


{% macro default__snapshot_check_column_exists(column_name, existing_columns) -%}
    {{ return(column_name in existing_columns) }}
{%- endmacro %}


-- funcsign: (model, string, string, config, bool) -> struct{unique_key: list[string]|string|none, updated_at: string, row_changed: string, scd_id: string, invalidate_hard_deletes: bool, hard_deletes: string}
{% macro snapshot_check_strategy(node, snapshotted_rel, current_rel, model_config, target_exists) %}
    {# The model_config parameter is no longer used, but is passed in anyway for compatibility. #}
    {% set check_cols_config = config.get('check_cols') %}
    {% set primary_key = config.get('unique_key') %}
    {% set hard_deletes = adapter.get_hard_deletes_behavior(config) %}
    {% set invalidate_hard_deletes = hard_deletes == 'invalidate' %}
    {% set updated_at = config.get('updated_at') or snapshot_get_time() %}

    {% set column_added = false %}

    {% set column_added, check_cols = snapshot_check_all_get_existing_columns(node, target_exists, check_cols_config) %} -- noqa: check_cols_config is a string|list[string]|none

    {%- set row_changed_expr -%}
    (
    {%- if column_added -%}
        {{ get_true_sql() }}
    {%- else -%}
    {%- for col in check_cols -%}
        {{ snapshotted_rel }}.{{ col }} != {{ current_rel }}.{{ col }}
        or
        (
            (({{ snapshotted_rel }}.{{ col }} is null) and not ({{ current_rel }}.{{ col }} is null))
            or
            ((not {{ snapshotted_rel }}.{{ col }} is null) and ({{ current_rel }}.{{ col }} is null))
        )
        {%- if not loop.last %} or {% endif -%}
    {%- endfor -%}
    {%- endif -%}
    )
    {%- endset %}

    {% set scd_args = api.Relation.scd_args(primary_key, updated_at) %} -- noqa: primary_key is a string|list[string]|none
    {% set scd_id_expr = snapshot_hash_arguments(scd_args) %}

    {% do return({
        "unique_key": primary_key,
        "updated_at": updated_at,
        "row_changed": row_changed_expr,
        "scd_id": scd_id_expr,
        "invalidate_hard_deletes": invalidate_hard_deletes,
        "hard_deletes": hard_deletes
    }) %}
{% endmacro %}
