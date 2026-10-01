
{% macro postgres__snapshot_merge_sql(target, source, insert_cols) -%}
    {%- set insert_cols_csv = insert_cols | join(', ') -%}

    {%- set columns = config.get("snapshot_table_column_names") or get_snapshot_table_column_names() -%}

    update {{ target }}
    set {{ columns.dbt_valid_to }} = DBT_INTERNAL_SOURCE.{{ columns.dbt_valid_to }}
    from {{ source }} as DBT_INTERNAL_SOURCE
    where DBT_INTERNAL_SOURCE.{{ columns.dbt_scd_id }}::text = {{ target }}.{{ columns.dbt_scd_id }}::text
      and DBT_INTERNAL_SOURCE.dbt_change_type::text in ('update'::text, 'delete'::text)
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
    where DBT_INTERNAL_SOURCE.dbt_change_type::text = 'insert'::text;
{% endmacro %}
