{% macro singlestore__snapshot_merge_sql(target, source, insert_cols) -%}
    {%- set insert_cols_csv = insert_cols | join(', ') -%}

    {%- set columns = config.get("snapshot_table_column_names") or get_snapshot_table_column_names() -%}

    update {{ target }}
    join {{ source }} as DBT_INTERNAL_SOURCE
      on DBT_INTERNAL_SOURCE.{{ columns.dbt_scd_id }} = {{ target }}.{{ columns.dbt_scd_id }}
    set {{ target }}.{{ columns.dbt_valid_to }} = DBT_INTERNAL_SOURCE.{{ columns.dbt_valid_to }}
    where DBT_INTERNAL_SOURCE.dbt_change_type in ('update', 'delete')
      {% if config.get("dbt_valid_to_current") %}
        and ({{ target }}.{{ columns.dbt_valid_to }} = {{ config.get('dbt_valid_to_current') }} or {{ target }}.{{ columns.dbt_valid_to }} is null);
      {% else %}
        and {{ target }}.{{ columns.dbt_valid_to }} is null;
      {% endif %}

    insert into {{ target }} ({{ insert_cols_csv }})
    select {% for column in insert_cols -%}
        DBT_INTERNAL_SOURCE.{{ column }} {%- if not loop.last %}, {%- endif %}
    {%- endfor %}
    from {{ source }} as DBT_INTERNAL_SOURCE
    where DBT_INTERNAL_SOURCE.dbt_change_type = 'insert';
{% endmacro %}
