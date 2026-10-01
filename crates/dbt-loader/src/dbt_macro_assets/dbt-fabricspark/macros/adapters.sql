
{% macro create_temporary_view(relation, compiled_code) -%}
  {{ return(adapter.dispatch('create_temporary_view', 'dbt')(relation, compiled_code)) }}
{%- endmacro -%}

{#-- We can't use temporary tables with `create ... as ()` syntax --#}
{% macro fabricspark__create_temporary_view(relation, compiled_code) -%}
    create or replace temporary view {{ relation }} as
      {{ compiled_code }}
{%- endmacro -%}


{% macro describe_table_extended_without_caching(table_name) %}
  {#-- Spark with iceberg tables don't work with show table extended for #}
  {#-- V2 iceberg tables #}
  {#-- https://issues.apache.org/jira/browse/SPARK-33393 #}
  {% call statement('describe_table_extended_without_caching', fetch_result=True) -%}
    describe extended {{ table_name }}
  {% endcall %}
  {% do return(load_result('describe_table_extended_without_caching').table) %}
{% endmacro %}

{% macro fabricspark__persist_docs(relation, model, for_relation, for_columns) -%}
  {% if for_columns and config.persist_column_docs() and model.columns %}
    {% do alter_column_comment(relation, model.columns) %}
  {% endif %}
{% endmacro %}



