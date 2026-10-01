{% macro get_replace_sql(existing_relation, target_relation, sql) %}
  {{- log('Applying REPLACE to: ' ~ existing_relation) -}}
  {% do return(adapter.dispatch('get_replace_sql', 'dbt')(existing_relation, target_relation, sql)) %}
{% endmacro %}

{% macro databricks__get_replace_sql(existing_relation, target_relation, sql) %}
  {# /* if safe_relation_replace, prefer renaming */ #}
  {% if target_relation.type == 'table' %}
    {{ exceptions.raise_not_implemented('get_replace_sql not implemented for target of table') }}
  {% endif %}

  {% if existing_relation.is_metric_view and target_relation.is_metric_view %}
    {{ return(get_replace_metric_view_sql(target_relation, sql)) }}
  {% endif %}

  {% set safe_replace = config.get('use_safer_relation_operations', False) | as_bool  %}
  {% set file_format = adapter.resolve_file_format(config) %}
  {% set is_replaceable = existing_relation.type == target_relation.type and existing_relation.can_be_replaced and file_format == "delta" %}

  {% if not safe_replace %}
    {# Prioritize 'create or replace' for speed #}
    {% if is_replaceable and existing_relation.is_view %}
      {{ return(get_replace_view_sql(target_relation, sql)) }}
    {% elif is_replaceable and existing_relation.is_table %}
      {{ return(get_replace_table_sql(target_relation, sql)) }}
    {% elif is_replaceable and existing_relation.is_materialized_view %}
      {{ return(get_replace_materialized_view_sql(target_relation, sql)) }}
    {% endif %}
  {% endif %}

  {# If safe_replace, then we know that anything that would have been caught above is instead caught here #}
  {% if target_relation.can_be_renamed and existing_relation.can_be_renamed %}
    {{ return(safely_replace(existing_relation, target_relation, sql)) }}
  {% elif target_relation.can_be_renamed %}
    {{ return(stage_then_replace(existing_relation, target_relation, sql)) }}
  {% elif existing_relation.can_be_renamed %}
    {{ return(backup_and_create_in_place(existing_relation, target_relation, sql)) }}
  {% else %}
    {{ return(drop_and_create(existing_relation, target_relation, sql)) }}
  {% endif %}
{% endmacro %}

{# Create target at a staging location, then rename existing, then rename target, then drop existing #}
{% macro safely_replace(existing_relation, target_relation, sql) %}
  {{ log('Using safely_replace') }}
  {% set staging_relation = make_staging_relation(target_relation, type='view') %}
  {{ drop_relation_if_exists(staging_relation) }}
  {% call statement(name="main") %}
    {{ get_create_sql(staging_relation, sql) }}
  {% endcall %}
  {{ create_backup(existing_relation) }}
  {{ return([
    get_rename_sql(staging_relation, existing_relation.render()),
    get_drop_backup_sql(existing_relation)
  ]) }}
{% endmacro %}

{# Stage the target relation, then drop and replace the existing relation #}
{% macro stage_then_replace(existing_relation, target_relation, sql) %}
  {{ log('Using stage_then_replace') }}
  {% set staging_relation = make_staging_relation(target_relation, type='view') %}
  {{ drop_relation_if_exists(staging_relation) }}
  {% call statement(name="main") %}
    {{ get_create_sql(staging_relation, sql) }}
  {% endcall %}

  {{ return([
    get_drop_sql(existing_relation),
    get_rename_sql(staging_relation, existing_relation.render()),
  ]) }}
{% endmacro %}

{# Backup the existing relation, then create the target relation in place #}
{% macro backup_and_create_in_place(existing_relation, target_relation, sql) %}
  {{ log('Using backup_and_create_in_place') }}
  {{ create_backup(existing_relation) }}
  {{ return([
    get_create_sql(target_relation, sql),
    get_drop_backup_sql(existing_relation)
  ]) }}
{% endmacro %}

{# Drop the existing relation, then create the target relation #}
{% macro drop_and_create(existing_relation, target_relation, sql) %}
  {{ log('Using drop_and_create') }}
  {{ return([
    get_drop_sql(existing_relation),
    get_create_sql(target_relation, sql)
  ]) }}
{% endmacro %}
