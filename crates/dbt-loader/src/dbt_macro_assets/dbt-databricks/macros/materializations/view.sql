{% materialization view, adapter='databricks' -%}
  {{ log("MATERIALIZING VIEW") }}
  {%- set existing_relation = load_relation_with_metadata(this) -%}
  {%- set target_relation = this.incorporate(type='view') -%}
  {% set grant_config = config.get('grants') %}
  {% set tags = config.get('databricks_tags') %}
  {% set sql = adapter.clean_sql(sql) %}

  {% if adapter.behavior.use_materialization_v2 %}
    {{ run_pre_hooks() }}
    {% if existing_relation %}
      {% if relation_should_be_altered(existing_relation) %}
        {% set configuration_changes = get_configuration_changes(existing_relation) %}
        {% if configuration_changes and configuration_changes.changes %}
          {% if configuration_changes.requires_full_refresh %}
            {{ log('Using replace_with_view') }}
            {{ replace_with_view(existing_relation, target_relation) }}
          {% else %}
            {{ log('Using alter_view') }}
            {{ log(configuration_changes.changes) }}
            {{ alter_view(target_relation, configuration_changes.changes) }}
          {% endif %}
        {% else %}
          {{ execute_no_op(target_relation) }}
        {% endif %}
      {% else %}
        {{ replace_with_view(existing_relation, target_relation) }}
      {% endif %}
    {% else %}
      {% call statement('main') -%}
        {{ get_create_view_as_sql(target_relation, sql) }}
      {%- endcall %}
      {{ apply_tags(target_relation, tags) }}
      {% set column_tags = adapter.get_column_tags_from_model(config.model) %}
      {% if column_tags and column_tags.set_column_tags %}
        {{ apply_column_tags(target_relation, column_tags) }}
      {% endif %}
    {% endif %}
    {% set should_revoke = should_revoke(exists_as_view, full_refresh_mode=True) %}
    {% do apply_grants(target_relation, grant_config, should_revoke=True) %}

    {{ run_post_hooks() }}

  {% else %}
    {{ run_hooks(pre_hooks) }}

    -- If there's a table with the same name and we weren't told to full refresh,
    -- that's an error. If we were told to full refresh, drop it. This behavior differs
    -- for Snowflake and BigQuery, so multiple dispatch is used.
    {%- if existing_relation is not none and not existing_relation.is_view -%}
      {{ handle_existing_table(should_full_refresh(), existing_relation) }}
    {%- endif -%}

    -- build model
    {% call statement('main') -%}
      {{ get_create_view_as_sql(target_relation, sql) }}
    {%- endcall %}

    {% set should_revoke = should_revoke(exists_as_view, full_refresh_mode=True) %}
    {% do apply_grants(target_relation, grant_config, should_revoke=True) %}

    {%- do apply_tags(target_relation, tags) -%}

    {% set column_tags = adapter.get_column_tags_from_model(config.model) %}
    {% if column_tags and column_tags.set_column_tags %}
      {{ apply_column_tags(target_relation, column_tags) }}
    {% endif %}

    {{ run_hooks(post_hooks) }}
  {% endif %}

  {{ return({'relations': [target_relation]}) }}

{%- endmaterialization %}

{% macro replace_with_view(existing_relation, target_relation) %}
  {% set sql = adapter.clean_sql(sql) %}
  {% set tags = config.get('databricks_tags') %}
  {{ execute_multiple_statements(get_replace_sql(existing_relation, target_relation, sql)) }}
  {%- do apply_tags(target_relation, tags) -%}
  
  {% set column_tags = adapter.get_column_tags_from_model(config.model) %}
  {% if column_tags and column_tags.set_column_tags %}
    {{ apply_column_tags(target_relation, column_tags) }}
  {% endif %}
{% endmacro %}

{% macro relation_should_be_altered(existing_relation) %}
  {% if should_full_refresh() %}
    {{ return(False) }}
  {% endif %}
  {% set update_via_alter = config.get('view_update_via_alter', False) | as_bool %}
  {% if (existing_relation.is_view or existing_relation.is_metric_view) and update_via_alter %}
    {% if existing_relation.is_hive_metastore() %}
      {{ exceptions.raise_compiler_error("Cannot update a view in the Hive metastore via ALTER VIEW. Please set `view_update_via_alter: false` in your model configuration.") }}
    {% endif %}
    {{ return(True) }}
  {% endif %}
  {{ return(False) }}
{% endmacro %}
