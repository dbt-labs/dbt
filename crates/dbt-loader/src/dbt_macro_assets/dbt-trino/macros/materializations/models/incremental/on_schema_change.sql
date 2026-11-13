{# Trino-specific schema change handling with nested ROW column synchronization.
   Plain-named sync_column_schemas overrides dbt-core's version in the adapter package. #}

{% macro sync_column_schemas(on_schema_change, target_relation, schema_changes_dict) %}

  {% set row_sync_dict = schema_changes_dict %}
  {% set sync_nested_columns = config.get('sync_nested_columns', false) %}

  {% if sync_nested_columns %}
    {% set row_sync_result = adapter.sync_row_columns(
        on_schema_change,
        target_relation,
        schema_changes_dict,
      ) %}
    {% if row_sync_result is not none %}
      {% set row_sync_dict = row_sync_result %}
    {% endif %}
  {% endif %}

  {%- set add_to_target_arr = row_sync_dict['source_not_in_target'] -%}
  {%- set remove_from_target_arr = row_sync_dict['target_not_in_source'] -%}
  {%- set new_target_types = row_sync_dict['new_target_types'] -%}

  {%- if on_schema_change == 'append_new_columns' -%}
    {%- if add_to_target_arr | length > 0 -%}
      {%- do alter_relation_add_remove_columns(target_relation, add_to_target_arr, none) -%}
    {%- endif -%}

  {% elif on_schema_change == 'sync_all_columns' %}

    {% if add_to_target_arr | length > 0 or remove_from_target_arr | length > 0 %}
      {%- do alter_relation_add_remove_columns(target_relation, add_to_target_arr, remove_from_target_arr) -%}
    {% endif %}

    {% if new_target_types != [] %}
      {% for ntt in new_target_types %}
        {% set column_name = ntt['column_name'] %}
        {% set new_type = ntt['new_type'] %}
        {% do alter_column_type(target_relation, column_name, new_type) %}
      {% endfor %}
    {% endif %}

  {% endif %}

  {% set schema_change_message %}
    In {{ target_relation }}:
        Schema change approach: {{ on_schema_change }}
        Columns added: {{ add_to_target_arr }}
        Columns removed: {{ remove_from_target_arr }}
        Data types changed: {{ new_target_types }}
  {% endset %}

  {% do log(schema_change_message) %}

{% endmacro %}
