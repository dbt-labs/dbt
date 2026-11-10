{% macro databricks__list_relations_without_caching(schema_relation) %}
  {{ return(adapter.get_relations_without_caching(schema_relation)) }}
{% endmacro %}

{% macro show_table_extended(schema_relation) %}
  {{ return(adapter.dispatch('show_table_extended', 'dbt')(schema_relation)) }}
{% endmacro %}

{% macro databricks__show_table_extended(schema_relation) %}
  {{ return(run_query_as(show_table_extended_sql(schema_relation), 'show_table_extended')) }}
{% endmacro %}

{% macro show_table_extended_sql(schema_relation) %}
SHOW TABLE EXTENDED IN {{ schema_relation.without_identifier()|lower }} LIKE '{{ schema_relation.identifier|lower }}'
{% endmacro %}

{% macro show_tables(relation) %}
  {{ return(adapter.dispatch('show_tables', 'dbt')(relation)) }}
{% endmacro %}

{% macro databricks__show_tables(relation) %}
  {{ return(run_query_as(show_tables_sql(relation), 'show_tables')) }}
{% endmacro %}

{% macro show_tables_sql(relation) %}

SHOW TABLES IN {{ relation.render() }}
{% endmacro %}

{% macro databricks__list_schemas(database) -%}
  {{ return(run_query_as(list_schemas_sql(database), 'list_schemas')) }}
{% endmacro %}

{% macro list_schemas_sql(database) %}
{% if database %}
  SHOW SCHEMAS IN {{ database }}
{% else %}
  SHOW SCHEMAS
{% endif %}
{% endmacro %}

{% macro databricks__check_schema_exists(database, schema) %}
  {{ return(run_query_as(check_schema_exists_sql(database, schema), 'check_schema_exists')) }}
{% endmacro %}

{% macro check_schema_exists_sql(database, schema) %}
  SHOW SCHEMAS IN {{ database }} LIKE '{{ schema }}'
{% endmacro %}

{% macro show_views(relation) %}
  {{ return(adapter.dispatch('show_views', 'dbt')(relation)) }}
{% endmacro %}

{% macro databricks__show_views(relation) %}
  {{ return(run_query_as(show_views_sql(relation), 'show_views')) }}
{% endmacro %}

{% macro show_views_sql(relation) %}
SHOW VIEWS IN {{ relation.render() }}
{% endmacro %}

{% macro databricks__get_relation_last_modified(information_schema, relations) -%}
  {% call statement('last_modified', fetch_result=True) %}
    {{ get_relation_last_modified_sql(information_schema, relations) }}
  {% endcall %}
  {{ return(load_result('last_modified')) }}
{% endmacro %}

{% macro get_relation_last_modified_sql(information_schema, relations) %}
  {% if information_schema.is_hive_metastore() %}
    {%- for relation in relations -%}
SELECT
  '{{ relation.schema }}' AS schema,
  '{{ relation.identifier }}' AS identifier,
  max(timestamp) AS last_modified,
  {{ current_timestamp() }} AS snapshotted_at
  FROM (DESCRIBE HISTORY {{ relation.render() }})
      {% if not loop.last %}
UNION ALL
      {% endif %}
    {%- endfor -%}
  {% else %}
SELECT
  table_schema AS schema,
  table_name AS identifier,
  last_altered AS last_modified,
  {{ current_timestamp() }} AS snapshotted_at
FROM `system`.`information_schema`.`tables`
WHERE table_catalog = '{{ information_schema.database|lower }}'
  AND (
    {%- for relation in relations -%}
    (table_schema = '{{ relation.schema|lower }}' AND
    table_name = '{{ relation.identifier|lower }}'){%- if not loop.last %} OR {% endif -%}
    {%- endfor -%}
  )
  {% endif %}
{% endmacro %}

{% macro get_view_description(relation) %}
  {{ return(run_query_as(get_view_description_sql(relation), 'get_view_description')) }}
{% endmacro %}

{% macro get_view_description_sql(relation) %}
SELECT *
FROM `system`.`information_schema`.`views`
WHERE table_catalog = '{{ relation.database|lower }}'
  AND table_schema = '{{ relation.schema|lower }}'
  AND table_name = '{{ relation.identifier|lower }}'
{% endmacro %}

{% macro get_uc_tables(relation) %}
  {{ return(run_query_as(get_uc_tables_sql(relation), 'get_uc_tables')) }}
{% endmacro %}

{% macro get_uc_tables_sql(relation) %}
SELECT
  table_name,
  if(table_type IN ('EXTERNAL', 'MANAGED', 'MANAGED_SHALLOW_CLONE', 'EXTERNAL_SHALLOW_CLONE'), 'table', lower(table_type)) AS table_type,
  lower(data_source_format) AS file_format,
  table_owner,
  if(
    table_type IN (
      'EXTERNAL',
      'MANAGED',
      'MANAGED_SHALLOW_CLONE',
      'EXTERNAL_SHALLOW_CLONE'
    ),
    lower(table_type),
    NULL
  ) AS databricks_table_type
FROM `system`.`information_schema`.`tables`
WHERE table_catalog = '{{ relation.database|lower }}' 
  AND table_schema = '{{ relation.schema|lower }}'
  {%- if relation.identifier %}
  AND table_name = '{{ relation.identifier|lower }}'
  {% endif %}
{% endmacro %}

{% macro describe_table_extended_as_json(relation) %}
  {{ return(run_query_as(describe_table_extended_as_json_sql(relation), 'describe_table_extended_as_json')) }}
{% endmacro %}

{% macro describe_table_extended_as_json_sql(relation) %}
DESCRIBE TABLE EXTENDED {{ relation.render() }} AS JSON
{% endmacro %}
