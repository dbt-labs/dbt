{% macro databricks__get_catalog(information_schema, schemas) -%}
  {% set query %}
WITH tables AS (
  {{ databricks__get_catalog_tables_sql(information_schema) }}
  {{ databricks__get_catalog_schemas_where_clause_sql(information_schema.database, schemas) }}
),
columns AS (
  {{ databricks__get_catalog_columns_sql(information_schema) }}
  {{ databricks__get_catalog_schemas_where_clause_sql(information_schema.database, schemas) }}
)
{{ databricks__get_catalog_results_sql() }}
  {%- endset -%}

  {{ return(run_query_as(query, 'get_catalog')) }}
{%- endmacro %}

{% macro databricks__get_catalog_relations(information_schema, relations) -%}
  {% set query %}
WITH tables AS (
  {{ databricks__get_catalog_tables_sql(information_schema) }}
  {{ databricks__get_catalog_relations_where_clause_sql(information_schema.database, relations) }}
),
columns AS (
  {{ databricks__get_catalog_columns_sql(information_schema) }}
  {{ databricks__get_catalog_relations_where_clause_sql(information_schema.database, relations) }}
)
{{ databricks__get_catalog_results_sql() }}
  {%- endset -%}

  {{ return(run_query_as(query, 'get_catalog_relations')) }}
{%- endmacro %}

{% macro databricks__get_catalog_tables_sql(information_schema) -%}
SELECT
  table_catalog AS table_database,
  table_schema,
  table_name,
  lower(table_type) AS table_type,
  comment AS table_comment,
  table_owner,
  'Last Modified' AS `stats:last_modified:label`,
  last_altered AS `stats:last_modified:value`,
  'The timestamp for last update/change' AS `stats:last_modified:description`,
  (last_altered IS NOT NULL AND table_type NOT ILIKE '%VIEW%') AS `stats:last_modified:include`
FROM `system`.`information_schema`.`tables`
{%- endmacro %}

{% macro databricks__get_catalog_columns_sql(information_schema) -%}
SELECT
  table_catalog AS table_database,
  table_schema,
  table_name,
  column_name,
  ordinal_position AS column_index,
  lower(full_data_type) AS column_type,
  comment AS column_comment
FROM `system`.`information_schema`.`columns`
{%- endmacro %}

{% macro databricks__get_catalog_results_sql() -%}
SELECT *
FROM tables
JOIN columns USING (table_database, table_schema, table_name)
ORDER BY column_index
{%- endmacro %}

{% macro databricks__get_catalog_schemas_where_clause_sql(catalog, schemas) -%}
WHERE table_catalog = '{{ catalog|lower }}' AND (
  {%- for schema in schemas -%}
  table_schema = '{{ schema|lower }}'{%- if not loop.last %} OR {% endif -%}
  {%- endfor -%})
{%- endmacro %}


{% macro databricks__get_catalog_relations_where_clause_sql(catalog, relations) -%}
WHERE table_catalog = '{{ catalog|lower }}' AND (
  {%- for relation in relations -%}
    {%- if relation.schema and relation.identifier %}
  (
    table_schema = '{{ relation.schema|lower }}'
    AND table_name = '{{ relation.identifier|lower }}'
  )
    {%- elif relation.schema %}
  (
    table_schema = '{{ relation.schema|lower }}'
  )
    {% else %}
      {% do exceptions.raise_compiler_error(
        '`get_catalog_relations` requires a list of relations, each with a schema'
      ) %}
    {% endif %}
    {%- if not loop.last %} OR {% endif -%}
  {%- endfor -%}
)
{%- endmacro %}
