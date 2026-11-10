{#
    Add new columns to the table if applicable
#}
-- funcsign: (relation, list[base_column]) -> string
{% macro create_columns(relation, columns) %}
  {{ adapter.dispatch('create_columns', 'dbt')(relation, columns) }}
{% endmacro %}

-- funcsign: (relation, list[base_column]) -> string
{% macro default__create_columns(relation, columns) %}
  {% for column in columns %}
    {% call statement() %}
      alter table {{ relation.render() }} add column {{ adapter.quote(column.name) }} {{ column.expanded_data_type }};
    {% endcall %}
  {% endfor %}
{% endmacro %}


-- funcsign: (relation) -> string
{% macro post_snapshot(staging_relation) %}
  {{ adapter.dispatch('post_snapshot', 'dbt')(staging_relation) }}
{% endmacro %}

-- funcsign: (relation) -> string
{% macro default__post_snapshot(staging_relation) %}
    {# no-op #}
{% endmacro %}

-- funcsign: () -> string
{% macro get_true_sql() %}
  {{ adapter.dispatch('get_true_sql', 'dbt')() }}
{% endmacro %}

-- funcsign: () -> string
{% macro default__get_true_sql() %}
    {{ return('TRUE') }}
{% endmacro %}

-- funcsign: (strategy, string, relation) -> string
{% macro snapshot_staging_table(strategy, source_sql, target_relation) -%}
  {{ adapter.dispatch('snapshot_staging_table', 'dbt')(strategy, source_sql, target_relation) }}
{% endmacro %}

-- funcsign: () -> struct{dbt_valid_to: string, dbt_valid_from: string, dbt_scd_id: string, dbt_updated_at: string, dbt_is_deleted: string}
{% macro get_snapshot_table_column_names() %}
    {{ return({'dbt_valid_to': 'dbt_valid_to', 'dbt_valid_from': 'dbt_valid_from', 'dbt_scd_id': 'dbt_scd_id', 'dbt_updated_at': 'dbt_updated_at', 'dbt_is_deleted': 'dbt_is_deleted'}) }}
{% endmacro %}

-- funcsign: (strategy, string, relation) -> string
{% macro default__snapshot_staging_table(strategy, source_sql, target_relation) -%}
    {% set columns = config.get('snapshot_table_column_names') or get_snapshot_table_column_names() %}
    {% if strategy.hard_deletes == 'new_record' %}
        {% set new_scd_id = snapshot_hash_arguments([columns.dbt_scd_id, snapshot_get_time()]) %}
    {% endif %}
    with snapshot_query as (

        {{ source_sql }}

    ),

    snapshotted_data as (

        select *, {{ unique_key_fields(strategy.unique_key) }}
        from {{ target_relation }}
        where
            {% if config.get('dbt_valid_to_current') %}
		{% set source_unique_key = columns.dbt_valid_to | trim %}
		{% set target_unique_key = config.get('dbt_valid_to_current') | trim %} -- noqa: optional[string] does not support trim

		{# The exact equals semantics between NULL values depends on the current behavior flag set. Also, update records if the source field is null #}
                ( {{ equals(source_unique_key, target_unique_key) }} or {{ source_unique_key }} is null )
            {% else %}
                {{ columns.dbt_valid_to }} is null
            {% endif %}

    ),

    insertions_source_data as (

        select *, {{ unique_key_fields(strategy.unique_key) }},
            {{ strategy.updated_at }} as {{ columns.dbt_updated_at }},
            {{ strategy.updated_at }} as {{ columns.dbt_valid_from }},
            {{ get_dbt_valid_to_current(strategy, columns) }},
            {{ strategy.scd_id }} as {{ columns.dbt_scd_id }}

        from snapshot_query
    ),

    updates_source_data as (

        select *, {{ unique_key_fields(strategy.unique_key) }},
            {{ strategy.updated_at }} as {{ columns.dbt_updated_at }},
            {{ strategy.updated_at }} as {{ columns.dbt_valid_from }},
            {{ strategy.updated_at }} as {{ columns.dbt_valid_to }}

        from snapshot_query
    ),

    {%- if strategy.hard_deletes == 'invalidate' or strategy.hard_deletes == 'new_record' %}

    deletes_source_data as (

        select *, {{ unique_key_fields(strategy.unique_key) }}
        from snapshot_query
    ),
    {% endif %}

    insertions as (

        select
            'insert' as dbt_change_type,
            source_data.*
          {%- if strategy.hard_deletes == 'new_record' -%}
            ,'False' as {{ columns.dbt_is_deleted }}
          {%- endif %}

        from insertions_source_data as source_data
        left outer join snapshotted_data
            on {{ unique_key_join_on(strategy.unique_key, "snapshotted_data", "source_data") }}
            where {{ unique_key_is_null(strategy.unique_key, "snapshotted_data") }}
            or ({{ unique_key_is_not_null(strategy.unique_key, "snapshotted_data") }} and (
               {{ strategy.row_changed }} {%- if strategy.hard_deletes == 'new_record' -%} or snapshotted_data.{{ columns.dbt_is_deleted }} = 'True' {% endif %}
            )

        )

    ),

    updates as (

        select
            'update' as dbt_change_type,
            source_data.*,
            snapshotted_data.{{ columns.dbt_scd_id }}
          {%- if strategy.hard_deletes == 'new_record' -%}
            , snapshotted_data.{{ columns.dbt_is_deleted }}
          {%- endif %}

        from updates_source_data as source_data
        join snapshotted_data
            on {{ unique_key_join_on(strategy.unique_key, "snapshotted_data", "source_data") }}
        where (
            {{ strategy.row_changed }}  {%- if strategy.hard_deletes == 'new_record' -%} or snapshotted_data.{{ columns.dbt_is_deleted }} = 'True' {% endif %}
        )
    )

    {%- if strategy.hard_deletes == 'invalidate' or strategy.hard_deletes == 'new_record' %}
    ,
    deletes as (

        select
            'delete' as dbt_change_type,
            source_data.*,
            {{ snapshot_get_time() }} as {{ columns.dbt_valid_from }},
            {{ snapshot_get_time() }} as {{ columns.dbt_updated_at }},
            {{ snapshot_get_time() }} as {{ columns.dbt_valid_to }},
            snapshotted_data.{{ columns.dbt_scd_id }}
          {%- if strategy.hard_deletes == 'new_record' -%}
            , snapshotted_data.{{ columns.dbt_is_deleted }}
          {%- endif %}
        from snapshotted_data
        left join deletes_source_data as source_data
            on {{ unique_key_join_on(strategy.unique_key, "snapshotted_data", "source_data") }}
            where {{ unique_key_is_null(strategy.unique_key, "source_data") }}

            {%- if strategy.hard_deletes == 'new_record' %}
            and not (
                --avoid updating the record's valid_to if the latest entry is marked as deleted
                snapshotted_data.{{ columns.dbt_is_deleted }} = 'True'
                and
                {% if config.get('dbt_valid_to_current') -%}
                    snapshotted_data.{{ columns.dbt_valid_to }} = {{ config.get('dbt_valid_to_current') }}
                {%- else -%}
                    snapshotted_data.{{ columns.dbt_valid_to }} is null
                {%- endif %}
            )
            {%- endif %}
    )
    {%- endif %}

    {%- if strategy.hard_deletes == 'new_record' %}
        {% set snapshotted_cols = get_list_of_column_names(get_columns_in_relation(target_relation)) %}
        {% set source_col_names = get_columns_in_query(source_sql) %}
    ,
    deletion_records as (

        select
            'insert' as dbt_change_type,
            {%- for col_name in source_col_names -%}
            {%- if col_name in snapshotted_cols -%}
            snapshotted_data.{{ adapter.quote(col_name) }},
            {%- else -%}
            source_data.{{ adapter.quote(col_name) }},
            {%- endif -%}
            {% endfor -%}
            {%- if strategy.unique_key | is_list -%}
                {%- for key in strategy.unique_key -%}
            snapshotted_data.{{ key }} as dbt_unique_key_{{ loop.index }},
                {% endfor -%}
            {%- else -%}
            snapshotted_data.dbt_unique_key as dbt_unique_key,
            {% endif -%}
            {{ snapshot_get_time() }} as {{ columns.dbt_valid_from }},
            {{ snapshot_get_time() }} as {{ columns.dbt_updated_at }},
            snapshotted_data.{{ columns.dbt_valid_to }} as {{ columns.dbt_valid_to }},
            {{ new_scd_id }} as {{ columns.dbt_scd_id }},
            'True' as {{ columns.dbt_is_deleted }}
        from snapshotted_data
        left join deletes_source_data as source_data
            on {{ unique_key_join_on(strategy.unique_key, "snapshotted_data", "source_data") }}
        where {{ unique_key_is_null(strategy.unique_key, "source_data") }}
        and not (
            --avoid inserting a new record if the latest one is marked as deleted
            snapshotted_data.{{ columns.dbt_is_deleted }} = 'True'
            and
            {% if config.get('dbt_valid_to_current') -%}
                snapshotted_data.{{ columns.dbt_valid_to }} = {{ config.get('dbt_valid_to_current') }}
            {%- else -%}
                snapshotted_data.{{ columns.dbt_valid_to }} is null
            {%- endif %}
            )

    )
    {%- endif %}

    select * from insertions
    union all
    select * from updates
    {%- if strategy.hard_deletes == 'invalidate' or strategy.hard_deletes == 'new_record' %}
    union all
    select * from deletes
    {%- endif %}
    {%- if strategy.hard_deletes == 'new_record' %}
    union all
    select * from deletion_records
    {%- endif %}


{%- endmacro %}


-- funcsign: (strategy, string) -> string
{% macro build_snapshot_table(strategy, sql) -%}
  {{ adapter.dispatch('build_snapshot_table', 'dbt')(strategy, sql) }}
{% endmacro %}

-- funcsign: (strategy, string) -> string
{% macro default__build_snapshot_table(strategy, sql) %}
    {% set columns = config.get('snapshot_table_column_names') or get_snapshot_table_column_names() %}

    select *,
        {{ strategy.scd_id }} as {{ columns.dbt_scd_id }},
        {{ strategy.updated_at }} as {{ columns.dbt_updated_at }},
        {{ strategy.updated_at }} as {{ columns.dbt_valid_from }},
        {{ get_dbt_valid_to_current(strategy, columns) }}
      {%- if strategy.hard_deletes == 'new_record' -%}
        , 'False' as {{ columns.dbt_is_deleted }}
      {% endif -%}
    from (
        {{ sql }}
    ) sbq

{% endmacro %}


-- funcsign: (strategy, string, relation) -> relation
{% macro build_snapshot_staging_table(strategy, sql, target_relation) %}
    {% set temp_relation = make_temp_relation(target_relation) %}

    {% set select = snapshot_staging_table(strategy, sql, target_relation) %}

    {% call statement('build_snapshot_staging_relation') %}
        {{ create_table_as(True, temp_relation, select) }}
    {% endcall %}

    {% do return(temp_relation) %}
{% endmacro %}


-- funcsign: (string) -> string
{% macro get_updated_at_column_data_type(snapshot_sql) %}
    {% set snapshot_sql_column_schema = get_column_schema_from_query(snapshot_sql) %}
    {% set dbt_updated_at_data_type = null %}
    {% set ns = namespace() -%} {#-- handle for-loop scoping with a namespace --#}
    {% set ns.dbt_updated_at_data_type = null -%}
    {% for column in snapshot_sql_column_schema %}
    {%   if ((column.column == 'dbt_updated_at') or (column.column == 'DBT_UPDATED_AT')) %}
    {%     set ns.dbt_updated_at_data_type = column.dtype %}
    {%   endif %}
    {% endfor %}
    {{ return(ns.dbt_updated_at_data_type or none)  }}
{% endmacro %}


-- funcsign: (string) -> string
{% macro check_time_data_types(sql) %}
  {% set dbt_updated_at_data_type = get_updated_at_column_data_type(sql) %}
  {% set snapshot_get_time_data_type = get_snapshot_get_time_data_type() %}
  {#- Assumes strategy.hard_deletes (which builds the SQL)
      agrees with get_hard_deletes_behavior(config); true for all built-in strategies. #}
  {% set hard_deletes = adapter.get_hard_deletes_behavior(config) %}
  {% if hard_deletes in ['invalidate', 'new_record'] and snapshot_get_time_data_type is not none and dbt_updated_at_data_type is not none and snapshot_get_time_data_type != dbt_updated_at_data_type %}
  {%   if exceptions.warn_snapshot_timestamp_data_types %}
  {{     exceptions.warn_snapshot_timestamp_data_types(snapshot_get_time_data_type, dbt_updated_at_data_type) }}
  {%   endif %}
  {% endif %}
{% endmacro %}


-- funcsign: (strategy, list[base_column]) -> string
{% macro get_dbt_valid_to_current(strategy, columns) %}
  {% set dbt_valid_to_current = config.get('dbt_valid_to_current') or "null" %}
  coalesce(nullif({{ strategy.updated_at }}, {{ strategy.updated_at }}), {{dbt_valid_to_current}})
  as {{ columns.dbt_valid_to }}
{% endmacro %}


-- funcsign: (string|list[string]|none) -> string
{% macro unique_key_fields(unique_key) %}
    {% if unique_key | is_list %}
        {% for key in unique_key %}
            {{ key }} as dbt_unique_key_{{ loop.index }}
            {%- if not loop.last %} , {%- endif %}
        {% endfor %}
    {% else %}
        {{ unique_key }} as dbt_unique_key
    {% endif %}
{% endmacro %}


-- funcsign: (string|list[string]|none, string, string) -> string
{% macro unique_key_join_on(unique_key, identifier, from_identifier) %}
    {% if unique_key | is_list %}
        {% for key in unique_key %}
	    {% set source_unique_key = (identifier ~ ".dbt_unique_key_" ~ loop.index) | trim %}
	    {% set target_unique_key = (from_identifier ~ ".dbt_unique_key_" ~ loop.index) | trim %}
	    {{ equals(source_unique_key, target_unique_key) }}
            {%- if not loop.last %} and {%- endif %}
        {% endfor %}
    {% else %}
        {{ identifier }}.dbt_unique_key = {{ from_identifier }}.dbt_unique_key
    {% endif %}
{% endmacro %}


-- funcsign: (string|list[string]|none, string) -> string
{% macro unique_key_is_null(unique_key, identifier) %}
    {% if unique_key | is_list %}
        {{ identifier }}.dbt_unique_key_1 is null
    {% else %}
        {{ identifier }}.dbt_unique_key is null
    {% endif %}
{% endmacro %}


-- funcsign: (string|list[string]|none, string) -> string
{% macro unique_key_is_not_null(unique_key, identifier) %}
    {% if unique_key | is_list %}
        {{ identifier }}.dbt_unique_key_1 is not null
    {% else %}
        {{ identifier }}.dbt_unique_key is not null
    {% endif %}
{% endmacro %}
