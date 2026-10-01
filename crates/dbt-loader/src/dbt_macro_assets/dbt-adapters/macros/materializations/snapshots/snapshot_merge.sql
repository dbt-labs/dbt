
-- funcsign: (relation, string, list[string]) -> string
{% macro snapshot_merge_sql(target, source, insert_cols) -%}
  {{ adapter.dispatch('snapshot_merge_sql', 'dbt')(target, source, insert_cols) }}
{%- endmacro %}


-- funcsign: (relation, string, list[string]) -> string
{% macro default__snapshot_merge_sql(target, source, insert_cols) -%}
    {%- set insert_cols_csv = insert_cols | join(', ') -%}

    {%- set columns = config.get("snapshot_table_column_names") or get_snapshot_table_column_names() -%}

    merge into {{ target.render() }} as DBT_INTERNAL_DEST
    using {{ source }} as DBT_INTERNAL_SOURCE
    on DBT_INTERNAL_SOURCE.{{ columns.dbt_scd_id }} = DBT_INTERNAL_DEST.{{ columns.dbt_scd_id }}

    when matched
     {% if config.get("dbt_valid_to_current") %}
	{% set source_unique_key = ("DBT_INTERNAL_DEST." ~ columns.dbt_valid_to) | trim %}
	{% set target_unique_key = config.get('dbt_valid_to_current') | trim %}
	and ({{ equals(source_unique_key, target_unique_key) }} or {{ source_unique_key }} is null)

     {% else %}
       and DBT_INTERNAL_DEST.{{ columns.dbt_valid_to }} is null
     {% endif %}
     and DBT_INTERNAL_SOURCE.dbt_change_type in ('update', 'delete')
        then update
        set {{ columns.dbt_valid_to }} = DBT_INTERNAL_SOURCE.{{ columns.dbt_valid_to }}

    when not matched
     and DBT_INTERNAL_SOURCE.dbt_change_type = 'insert'
        then insert ({{ insert_cols_csv }})
        values ({{ insert_cols_csv }})

{% endmacro %}
