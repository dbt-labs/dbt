{# BigQuery-specific schema change handling that augments the core macro with
# STRUCT column synchronization logic. #}
{% macro bigquery__check_for_schema_changes(source_relation, target_relation) %}

  {% set schema_changed = False %}

  {%- set source_columns = adapter.get_columns_in_relation(source_relation) -%}
  {%- set target_columns = adapter.get_columns_in_relation(target_relation) -%}
  {%- set source_not_in_target = diff_columns(source_columns, target_columns) -%}
  {%- set target_not_in_source = diff_columns(target_columns, source_columns) -%}

  {% set new_target_types = diff_column_data_types(source_columns, target_columns) %}

  {% if source_not_in_target != [] %}
    {% set schema_changed = True %}
  {% elif target_not_in_source != [] or new_target_types != [] %}
    {% set schema_changed = True %}
  {% elif new_target_types != [] %}
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

  {% do changes_dict.update({'source_relation': source_relation}) %}

  {{ return(changes_dict) }}

{% endmacro %}

{# DIVERGENCE: FIXME:
   Fusion is missing the adapter.sync_struct_columns method.
   Only uncomment this when this is implemented.
{% macro bigquery__process_schema_changes(on_schema_change, source_relation, target_relation) %}

  {% if on_schema_change == 'ignore' %}

    {{ return({}) }}

  {% else %}

  {% set schema_changes_dict = check_for_schema_changes(source_relation, target_relation) %}
  {% do schema_changes_dict.update({'source_relation': source_relation}) %}

    {% if schema_changes_dict['schema_changed'] %}

      {% if on_schema_change == 'fail' %}

        {% set fail_msg %}
            The source and target schemas on this incremental model are out of sync!
            They can be reconciled in several ways:
              - set the `on_schema_change` config to either append_new_columns or sync_all_columns, depending on your situation.
              - Re-run the incremental model with `full_refresh: True` to update the target schema.
              - update the schema manually and re-run the process.

            Additional troubleshooting context:
               Source columns not in target: {{ schema_changes_dict['source_not_in_target'] }}
               Target columns not in source: {{ schema_changes_dict['target_not_in_source'] }}
               New column types: {{ schema_changes_dict['new_target_types'] }}
        {% endset %}

        {% do exceptions.raise_compiler_error(fail_msg) %}

      {% else %}

        {% do bigquery__sync_column_schemas(on_schema_change, target_relation, schema_changes_dict) %}

      {% endif %}

    {% endif %}

    {{ return(schema_changes_dict['source_columns']) }}

  {% endif %}

{% endmacro %}
#}

{# DIVERGENCE: FIXME:
   Fusion is missing the adapter.sync_struct_columns method.
   Only uncomment this when this is implemented.
{% macro bigquery__sync_column_schemas(on_schema_change, target_relation, schema_changes_dict) %}
  {% set struct_sync_dict = schema_changes_dict %}
  {% set source_relation = schema_changes_dict.get('source_relation') %}

  {% if source_relation is not none %}
  {% set struct_sync_result = adapter.sync_struct_columns(
      on_schema_change,
      source_relation,
      target_relation,
      schema_changes_dict,
    ) %}
    {% if struct_sync_result is not none %}
      {% set struct_sync_dict = struct_sync_result %}
    {% endif %}
  {% endif %}

  {%- set add_to_target_arr = struct_sync_dict['source_not_in_target'] -%}

  {%- if on_schema_change == 'append_new_columns' -%}
    {%- if add_to_target_arr | length > 0 -%}
      {%- do alter_relation_add_remove_columns(target_relation, add_to_target_arr, none) -%}
    {%- endif -%}

  {% elif on_schema_change == 'sync_all_columns' %}
    {%- set remove_from_target_arr = struct_sync_dict['target_not_in_source'] -%}
    {%- set new_target_types = struct_sync_dict['new_target_types'] -%}

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
        Columns removed: {{ struct_sync_dict['target_not_in_source'] }}
        Data types changed: {{ struct_sync_dict['new_target_types'] }}
  {% endset %}

  {% do log(schema_change_message) %}

  {% do struct_sync_dict.pop('source_relation', none) %}
{% endmacro %}
#}