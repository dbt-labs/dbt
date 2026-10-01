{% macro duckdb__snapshot_merge_sql(target, source, insert_cols) -%}
    {%- set insert_cols_csv = insert_cols | join(', ') -%}

    {%- set columns = config.get("snapshot_table_column_names") or get_snapshot_table_column_names() -%}

    update {{ target }} as DBT_INTERNAL_TARGET
    set {{ columns.dbt_valid_to }} = DBT_INTERNAL_SOURCE.{{ columns.dbt_valid_to }}
    from {{ source }} as DBT_INTERNAL_SOURCE
    where DBT_INTERNAL_SOURCE.{{ columns.dbt_scd_id }}::text = DBT_INTERNAL_TARGET.{{ columns.dbt_scd_id }}::text
      and DBT_INTERNAL_SOURCE.dbt_change_type::text in ('update'::text, 'delete'::text)
      {% if config.get("dbt_valid_to_current") %}
        and (DBT_INTERNAL_TARGET.{{ columns.dbt_valid_to }} = {{ config.get('dbt_valid_to_current') }} or DBT_INTERNAL_TARGET.{{ columns.dbt_valid_to }} is null);
      {% else %}
        and DBT_INTERNAL_TARGET.{{ columns.dbt_valid_to }} is null;
      {% endif %}

    insert into {{ target }} ({{ insert_cols_csv }})
    select {% for column in insert_cols -%}
        DBT_INTERNAL_SOURCE.{{ column }} {%- if not loop.last %}, {%- endif %}
    {%- endfor %}
    from {{ source }} as DBT_INTERNAL_SOURCE
    where DBT_INTERNAL_SOURCE.dbt_change_type::text = 'insert'::text;

{% endmacro %}

{% macro build_snapshot_staging_table(strategy, sql, target_relation) %}
    {% set temp_relation = make_temp_relation(target_relation) %}

    {% set select = snapshot_staging_table(strategy, sql, target_relation) %}

    {% call statement('build_snapshot_staging_relation') %}
        {{ create_table_as(False, temp_relation, select) }}
    {% endcall %}

    {% do return(temp_relation) %}
{% endmacro %}

{% macro duckdb__post_snapshot(staging_relation) %}
    {% do return(drop_relation(staging_relation)) %}
{% endmacro %}
