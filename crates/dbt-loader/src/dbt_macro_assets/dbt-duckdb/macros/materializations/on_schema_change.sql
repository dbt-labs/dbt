{# DuckDB-specific schema change detection for incremental models.

   For SQL models the temp table does not exist at schema-check time (it is
   batched into a multi-statement execute together with the strategy DML), so
   we introspect source columns via get_column_schema_from_query(compiled_code)
   instead of adapter.get_columns_in_relation(source_relation).

   For Python models the temp table has already been materialised by a prior
   statement() call, so the default adapter.get_columns_in_relation path works. #}
{% macro duckdb__check_for_schema_changes(source_relation, target_relation) %}

  {% set schema_changed = False %}

  {%- if model['language'] != 'python' -%}
    {%- set source_columns = get_column_schema_from_query(compiled_code) -%}
  {%- else -%}
    {%- set source_columns = adapter.get_columns_in_relation(source_relation) -%}
  {%- endif -%}

  {%- set target_columns = adapter.get_columns_in_relation(target_relation) -%}
  {%- set source_not_in_target = diff_columns(source_columns, target_columns) -%}
  {%- set target_not_in_source = diff_columns(target_columns, source_columns) -%}

  {% set new_target_types = diff_column_data_types(source_columns, target_columns) %}

  {% if source_not_in_target != [] or target_not_in_source != [] or new_target_types != [] %}
    {% set schema_changed = True %}
  {% endif %}

  {% set changes_dict = {
    'schema_changed': schema_changed,
    'source_not_in_target': source_not_in_target,
    'target_not_in_source': target_not_in_source,
    'source_columns': source_columns,
    'target_columns': target_columns,
    'new_target_types': new_target_types
  } %}

  {% set msg %}
    In {{ target_relation }}:
        Schema changed: {{ schema_changed }}
        Source columns not in target: {{ source_not_in_target }}
        Target columns not in source: {{ target_not_in_source }}
        New column types: {{ new_target_types }}
  {% endset %}

  {% do log(msg) %}

  {{ return(changes_dict) }}

{% endmacro %}
