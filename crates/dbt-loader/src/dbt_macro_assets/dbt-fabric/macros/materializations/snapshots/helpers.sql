{% macro fabric__post_snapshot(staging_relation) %}
  -- Clean up the snapshot temp table
  {% do drop_relation_if_exists(staging_relation) %}
{% endmacro %}

{% macro fabric__create_columns(relation, columns) %}
  {% for column in columns %}
    {% call statement() %}
      alter table {{ relation.render() }} add "{{ column.name }}" {{ column.data_type }} NULL;
    {% endcall %}
  {% endfor %}
{% endmacro %}

{% macro fabric__get_true_sql() %}
  {{ return('1=1') }}
{% endmacro %}


{% macro fabric__build_snapshot_table(strategy, relation) %}
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
        select * from {{ relation }}
    ) sbq

{% endmacro %}

{% macro fabric__snapshot_staging_table(strategy, temp_snapshot_relation, target_relation) -%}

    {% set columns = config.get('snapshot_table_column_names') or get_snapshot_table_column_names() %}

    with snapshot_query as (
        select * from {{ temp_snapshot_relation }}
    ),
    snapshotted_data as (
        select *,
        {{ unique_key_fields(strategy.unique_key) }}
        from {{ target_relation }}
        where
        {% if config.get('dbt_valid_to_current') %}
            {# Check for either dbt_valid_to_current OR null, in order to correctly update records with nulls #}
            ( {{ columns.dbt_valid_to }} = {{ config.get('dbt_valid_to_current') }} or {{ columns.dbt_valid_to }} is null)
        {% else %}
            {{ columns.dbt_valid_to }} is null
        {% endif %}
        {%- if strategy.hard_deletes == 'new_record' -%}
            and {{ columns.dbt_is_deleted }} = 'False'
        {% endif -%}
    ),
    insertions_source_data as (
        select *,
        {{ unique_key_fields(strategy.unique_key) }},
        {{ strategy.updated_at }} as {{ columns.dbt_updated_at }},
        {{ strategy.updated_at }} as {{ columns.dbt_valid_from }},
        {{ get_dbt_valid_to_current(strategy, columns) }},
        {{ strategy.scd_id }} as {{ columns.dbt_scd_id }}
        from snapshot_query
    ),
    updates_source_data as (
        select *,
        {{ unique_key_fields(strategy.unique_key) }},
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
        select 'insert' as dbt_change_type, source_data.*
        {%- if strategy.hard_deletes == 'new_record' -%}
        ,'False' as {{ columns.dbt_is_deleted }}
        {%- endif %}
        from insertions_source_data as source_data
        left outer join snapshotted_data
            on {{ unique_key_join_on(strategy.unique_key, "snapshotted_data", "source_data") }}
            where {{ unique_key_is_null(strategy.unique_key, "snapshotted_data") }}
            or ({{ unique_key_is_not_null(strategy.unique_key, "snapshotted_data") }} and ({{ strategy.row_changed }}))
    ),
    updates as (
        select 'update' as dbt_change_type, source_data.*,
        snapshotted_data.{{ columns.dbt_scd_id }}
        {%- if strategy.hard_deletes == 'new_record' -%}
        , snapshotted_data.{{ columns.dbt_is_deleted }}
        {%- endif %}
        from updates_source_data as source_data
        join snapshotted_data
        on {{ unique_key_join_on(strategy.unique_key, "snapshotted_data", "source_data") }}
        where ({{ strategy.row_changed }})
    )
    {%- if strategy.hard_deletes == 'invalidate' or strategy.hard_deletes == 'new_record' %}
        ,
        deletes as (
            select 'delete' as dbt_change_type,
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
        )
    {%- endif %}
    {%- if strategy.hard_deletes == 'new_record' %}
        {%set source_query = "select * from "~temp_snapshot_relation%}
        {% set source_sql_cols = get_column_schema_from_query(source_query) %}
        ,
        deletion_records as (

            select
            'insert' as dbt_change_type,
            {%- for col in source_sql_cols -%}
            snapshotted_data.{{ adapter.quote(col.column) }},
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
            snapshotted_data.{{ columns.dbt_scd_id }},
            'True' as {{ columns.dbt_is_deleted }}
            from snapshotted_data
            left join deletes_source_data as source_data
            on {{ unique_key_join_on(strategy.unique_key, "snapshotted_data", "source_data") }}
            where {{ unique_key_is_null(strategy.unique_key, "source_data") }}
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

{% macro build_snapshot_staging_table(strategy, temp_snapshot_relation, target_relation) %}
    {% set temp_relation = make_temp_relation(target_relation) %}
    {{ adapter.drop_relation(temp_relation) }}

    {% set select = snapshot_staging_table(strategy, temp_snapshot_relation, target_relation) %}

    {% set tmp_tble_vw_relation = temp_relation.incorporate(path={"identifier": temp_relation.identifier ~ '__dbt_tmp_vw'}, type='view')-%}
    -- Dropping temp view relation if it exists
    {{ adapter.drop_relation(tmp_tble_vw_relation) }}

    {% call statement('build_snapshot_staging_relation') %}
        {{ get_create_table_as_sql(True, temp_relation, select) }}
    {% endcall %}

    -- Dropping temp view relation if it exists
    {{ adapter.drop_relation(tmp_tble_vw_relation) }}

    {% do return(temp_relation) %}
{% endmacro %}
