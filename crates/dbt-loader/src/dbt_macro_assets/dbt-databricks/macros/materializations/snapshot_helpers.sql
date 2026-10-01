{% macro databricks__snapshot_merge_sql(target, source, insert_cols) -%}
    {%- set insert_cols_csv = insert_cols | join(', ') -%}
    {# Get the hard_deletes configuration from config #}
    {%- set hard_deletes = config.get('hard_deletes', 'ignore') -%}
    {%- set invalidate_hard_deletes = (hard_deletes == 'invalidate') -%}

    {# Get column names configuration #}
    {%- set columns = config.get("snapshot_table_column_names") or get_snapshot_table_column_names() -%}

    merge into {{ target }} as DBT_INTERNAL_DEST
    {%- if target.is_iceberg %}
        {# create view only supports a name (no catalog, or schema) #}
        using {{ source.identifier }} as DBT_INTERNAL_SOURCE
    {%- else %}
        using {{ source }} as DBT_INTERNAL_SOURCE
    {%- endif %}
    on DBT_INTERNAL_SOURCE.{{ adapter.quote(columns.dbt_scd_id) }} = DBT_INTERNAL_DEST.{{ adapter.quote(columns.dbt_scd_id) }}
    when matched
    {%- if config.get("dbt_valid_to_current") %}
        and ( DBT_INTERNAL_DEST.{{ adapter.quote(columns.dbt_valid_to) }} = {{ config.get('dbt_valid_to_current') }} or
                DBT_INTERNAL_DEST.{{ adapter.quote(columns.dbt_valid_to) }} is null )
    {%- else %}
        and DBT_INTERNAL_DEST.{{ adapter.quote(columns.dbt_valid_to) }} is null
    {%- endif %}
        and DBT_INTERNAL_SOURCE.{{ adapter.quote('dbt_change_type') }} in ('update', 'delete')
        then update
        set {{ adapter.quote(columns.dbt_valid_to) }} = DBT_INTERNAL_SOURCE.{{ adapter.quote(columns.dbt_valid_to) }}

    when not matched
        and DBT_INTERNAL_SOURCE.{{ adapter.quote('dbt_change_type') }} = 'insert'
        then insert ({{ insert_cols_csv }})
        values ({{ insert_cols_csv }})
    ;
{% endmacro %}


{% macro databricks__create_columns(relation, columns) %}
    {%- if columns|length > 0 %}
    {%- call statement() %}
        alter table {{ relation }} add columns (
            {%- for column in columns %}
            {{ adapter.quote(column.name) }} {{ column.data_type }} {{- ',' if not loop.last -}}
            {%- endfor %}
        );
    {%- endcall %}
    {%- endif %}
{% endmacro %}


{% macro databricks__build_snapshot_table(strategy, sql) %}
    {%- set columns = config.get("snapshot_table_column_names") or get_snapshot_table_column_names() -%}
    {%- set hard_deletes = strategy.hard_deletes -%}

    select *,
        {{ strategy.scd_id }} as {{ columns.dbt_scd_id }},
        {{ strategy.updated_at }} as {{ columns.dbt_updated_at }},
        {{ strategy.updated_at }} as {{ columns.dbt_valid_from }},
        {{ get_dbt_valid_to_current(strategy, columns) }}
        {%- if hard_deletes == 'new_record' -%}
        , false as {{ columns.dbt_is_deleted }}
        {%- endif %}
    from (
        {{ sql }}
    ) sbq
{% endmacro %}

{% macro databricks__snapshot_staging_table(strategy, source_sql, target_relation) -%}
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
                {% set target_unique_key = config.get('dbt_valid_to_current') | trim %}

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
            , false as {{ columns.dbt_is_deleted }}
            {%- endif %}

        from insertions_source_data as source_data
        left outer join snapshotted_data
            on {{ unique_key_join_on(strategy.unique_key, "snapshotted_data", "source_data") }}
            where {{ unique_key_is_null(strategy.unique_key, "snapshotted_data") }}
            or ({{ unique_key_is_not_null(strategy.unique_key, "snapshotted_data") }} and (
                {{ strategy.row_changed }} {%- if strategy.hard_deletes == 'new_record' -%} or snapshotted_data.{{ columns.dbt_is_deleted }} = true {% endif %}
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
            {{ strategy.row_changed }} {%- if strategy.hard_deletes == 'new_record' -%} or snapshotted_data.{{ columns.dbt_is_deleted }} = true {% endif %}
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
                snapshotted_data.{{ columns.dbt_is_deleted }} = true
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
        {# Databricks-specific: Extract column names from agate.Row tuples #}
        {% set target_columns_raw = get_columns_in_relation(target_relation) %}
        {% set snapshotted_cols = [] %}
        {% for row in target_columns_raw %}
            {# agate.Row is a tuple: (col_name, data_type, comment) #}
            {# Filter out Databricks metadata rows (starting with # or empty) #}
            {% set col_name = row[0] %}
            {% if col_name and not col_name.startswith('#') %}
                {% do snapshotted_cols.append(col_name) %}
            {% endif %}
        {% endfor %}
        {% set source_sql_cols = get_column_schema_from_query(source_sql) %}
    ,
    deletion_records as (

        select
            'insert' as dbt_change_type,
            {#
                If a column has been added to the source it won't yet exist in the
                snapshotted table so we insert a null value as a placeholder for the column.
            #}
            {%- for col in source_sql_cols -%}
            {%- if col.name in snapshotted_cols -%}
            snapshotted_data.{{ adapter.quote(col.column) }},
            {%- else -%}
            NULL as {{ adapter.quote(col.column) }},
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
            true as {{ columns.dbt_is_deleted }}
        from snapshotted_data
        left join deletes_source_data as source_data
            on {{ unique_key_join_on(strategy.unique_key, "snapshotted_data", "source_data") }}
        where {{ unique_key_is_null(strategy.unique_key, "source_data") }}
        and not (
            -- avoid inserting a new record if the latest one is marked as deleted
            snapshotted_data.{{ columns.dbt_is_deleted }} = true
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
