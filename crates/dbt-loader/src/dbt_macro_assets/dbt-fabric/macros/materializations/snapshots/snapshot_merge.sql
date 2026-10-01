{% macro fabric__snapshot_merge_sql(target, source, insert_cols) %}

  {%- set insert_cols_csv = insert_cols | join(', ') -%}
  {%- set columns = config.get("snapshot_table_column_names") or get_snapshot_table_column_names() -%}
  {%- set target_table = target.include(database=False) -%}
  {%- set source_table = source.include(database=False) -%}
  {% set target_columns_list = [] %}
  {% for column in insert_cols %}
    {% set target_columns_list = target_columns_list.append("DBT_INTERNAL_SOURCE."+column)  %}
  {% endfor %}
  {%- set target_columns = target_columns_list | join(', ') -%}

  update DBT_INTERNAL_DEST
  set {{ columns.dbt_valid_to }} = DBT_INTERNAL_SOURCE.{{ columns.dbt_valid_to }}
  from {{ target_table }} as DBT_INTERNAL_DEST
  inner join {{ source_table }} as DBT_INTERNAL_SOURCE
  on DBT_INTERNAL_SOURCE.{{ columns.dbt_scd_id }} = DBT_INTERNAL_DEST.{{ columns.dbt_scd_id }}
  where DBT_INTERNAL_SOURCE.dbt_change_type in ('update', 'delete')
  {% if config.get("dbt_valid_to_current") %}
    and (DBT_INTERNAL_DEST.{{ columns.dbt_valid_to }} = {{ config.get('dbt_valid_to_current') }} or DBT_INTERNAL_DEST.{{ columns.dbt_valid_to }} is null)
  {% else %}
    and DBT_INTERNAL_DEST.{{ columns.dbt_valid_to }} is null
  {% endif %}
  {{ apply_label() }}

  insert into {{ target_table }} ({{ insert_cols_csv }})
  select {{target_columns}} from {{ source_table }} as DBT_INTERNAL_SOURCE
  where  DBT_INTERNAL_SOURCE.dbt_change_type = 'insert'
  {{ apply_label() }}
{% endmacro %}
