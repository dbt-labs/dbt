{% macro alter_metric_view(target_relation, changes) %}
  {{ log("Updating metric view via ALTER") }}
  {{ adapter.dispatch('alter_metric_view', 'dbt')(target_relation, changes) }}
{% endmacro %}

{% macro databricks__alter_metric_view(target_relation, changes) %}
  {% set tags = changes.get("tags") %}
  {% set tblproperties = changes.get("tblproperties") %}
  {% set query = changes.get("query") %}

  {# Handle YAML definition changes via ALTER VIEW AS #}
  {% if query %}
    {% call statement('main') %}
      {{ get_alter_metric_view_as_sql(target_relation, query.query) }}
    {% endcall %}
  {% else %}
    {# Ensure statement('main') is called for dbt to track the run #}
    {% call statement('main') %}
      select 1
    {% endcall %}
  {% endif %}

  {% if tags %}
    {{ apply_tags(target_relation, tags.set_tags) }}
  {% endif %}
  {% if tblproperties %}
    {{ apply_tblproperties(target_relation, tblproperties.tblproperties) }}
  {% endif %}
{% endmacro %}

{% macro get_alter_metric_view_as_sql(relation, yaml_content) -%}
  {{ adapter.dispatch('get_alter_metric_view_as_sql', 'dbt')(relation, yaml_content) }}
{%- endmacro %}

{% macro databricks__get_alter_metric_view_as_sql(relation, yaml_content) %}
{%- set yaml_body = adapter.yaml_quote_backtick_values(yaml_content) -%}
alter view {{ relation.render() }} as $$
{{ yaml_body }}
$$
{% endmacro %}

{% macro replace_with_metric_view(existing_relation, target_relation) %}
  {% set sql = adapter.clean_sql(sql) %}
  {% set tags = config.get('databricks_tags') %}
  {% set tblproperties = config.get('tblproperties') %}
  {{ execute_multiple_statements(get_replace_sql(existing_relation, target_relation, sql)) }}
  {%- do apply_tags(target_relation, tags) -%}

  {% if tblproperties %}
    {{ apply_tblproperties(target_relation, tblproperties) }}
  {% endif %}
{% endmacro %}
