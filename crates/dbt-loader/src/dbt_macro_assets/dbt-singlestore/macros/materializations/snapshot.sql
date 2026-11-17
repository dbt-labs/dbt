{#
  SingleStore snapshot materialization support.

  SingleStore does not have a MERGE statement, so the default dbt snapshot
  materialization dispatches to `singlestore__snapshot_merge_sql` which
  implements the merge semantics using two statements:
    1. UPDATE ... JOIN ... SET dbt_valid_to = ...
    2. INSERT INTO ... SELECT ... WHERE dbt_change_type = 'insert'

  The default `materialization snapshot` (from dbt-adapters) works out of the
  box for SingleStore because it calls `snapshot_merge_sql`, which dispatches
  to our `singlestore__snapshot_merge_sql` override below.

  What this file provides:
  - `singlestore__snapshot_hash_arguments`: uses MD5() for SCD id generation
  - `singlestore__post_snapshot`:           drops the staging table after use
  - `singlestore__snapshot_staging_table`:  the default staging SQL, with
                                            SingleStore comments and DATETIME casts
#}

{# Use SingleStore's built-in MD5() for snapshot SCD hash generation.
   `||` is logical OR in SingleStore (as in MySQL), so join with concat(). #}
{% macro singlestore__snapshot_hash_arguments(args) -%}
  md5(concat({%- for arg in args -%}
    coalesce(cast({{ arg }} as char), '')
    {% if not loop.last %}, '|', {% endif %}
  {%- endfor -%}))
{%- endmacro %}

{# Drop the temporary staging table created during snapshot execution #}
{% macro singlestore__post_snapshot(staging_relation) %}
    {{ drop_relation_if_exists(staging_relation) }}
{% endmacro %}

{# SingleStore CTAS infers untyped null or string literals in coalesce() as VARCHAR.
   Cast to datetime so dbt_valid_to is created as DATETIME matching upstream dbt-singlestore:
   https://github.com/memsql/dbt-singlestore/blob/master/dbt/include/singlestore/macros/materializations/snapshot/helpers.sql#L250-L277 #}
{% macro singlestore__get_dbt_valid_to_current(strategy, columns) %}
  {% set dbt_valid_to_current = config.get('dbt_valid_to_current') or "null" %}
  (coalesce(
    nullif({{ strategy.updated_at }}, {{ strategy.updated_at }}),
    ({{ dbt_valid_to_current }}) :> datetime)
    :> datetime)
  as {{ columns.dbt_valid_to }}
{% endmacro %}

{% macro singlestore__build_snapshot_table(strategy, sql) %}
    {% set columns = config.get('snapshot_table_column_names') or get_snapshot_table_column_names() %}

    select *,
        {{ strategy.scd_id }} as {{ columns.dbt_scd_id }},
        {{ strategy.updated_at }} as {{ columns.dbt_updated_at }},
        {{ strategy.updated_at }} as {{ columns.dbt_valid_from }},
        {{ singlestore__get_dbt_valid_to_current(strategy, columns) }}
      {%- if strategy.hard_deletes == 'new_record' -%}
        , 'False' as {{ columns.dbt_is_deleted }}
      {% endif -%}
    from (
        {{ sql }}
    ) sbq

{% endmacro %}

{# `default__snapshot_staging_table` with two SingleStore changes: the comments
   use `-- ` (MySQL-family engines only start a comment at `--` followed by
   whitespace), and dbt_valid_to comes from `singlestore__get_dbt_valid_to_current`
   so CTAS types it as DATETIME. #}
-- funcsign: (strategy, string, relation) -> string
{% macro singlestore__snapshot_staging_table(strategy, source_sql, target_relation) -%}
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
            {{ singlestore__get_dbt_valid_to_current(strategy, columns) }},
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
                -- avoid updating the record's valid_to if the latest entry is marked as deleted
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
            -- avoid inserting a new record if the latest one is marked as deleted
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
