-- funcsign: (relation, list[relation]) -> list[relation]
{% macro get_catalog_relations(information_schema, relations) -%}
  {{ return(adapter.dispatch('get_catalog_relations', 'dbt')(information_schema, relations)) }}
{%- endmacro %}

-- funcsign: (relation, list[relation]) -> list[relation]
{% macro default__get_catalog_relations(information_schema, relations) -%}
  {% set typename = adapter.type() %}
  {% set msg -%}
    get_catalog_relations not implemented for {{ typename }}
  {%- endset %}

  {{ exceptions.raise_compiler_error(msg) }}
{%- endmacro %}

-- funcsign: (relation, list[string]) -> agate_table
{% macro get_catalog(information_schema, schemas) -%}
  {{ return(adapter.dispatch('get_catalog', 'dbt')(information_schema, schemas)) }}
{%- endmacro %}

-- funcsign: (relation, list[string]) -> agate_table
{% macro default__get_catalog(information_schema, schemas) -%}

  {% set typename = adapter.type() %}
  {% set msg -%}
    get_catalog not implemented for {{ typename }}
  {%- endset %}

  {{ exceptions.raise_compiler_error(msg) }}
{% endmacro %}


-- funcsign: (string) -> string
{% macro information_schema_name(database) %}
  {{ return(adapter.dispatch('information_schema_name', 'dbt')(database)) }}
{% endmacro %}

-- funcsign: (string) -> string
{% macro default__information_schema_name(database) -%}
  {%- if database -%}
    {{ database }}.INFORMATION_SCHEMA
  {%- else -%}
    INFORMATION_SCHEMA
  {%- endif -%}
{%- endmacro %}


-- funcsign: (string) -> agate_table
{% macro list_schemas(database) -%}
  {{ return(adapter.dispatch('list_schemas', 'dbt')(database)) }}
{% endmacro %}

-- funcsign: (string) -> agate_table
{% macro default__list_schemas(database) -%}
  {% set sql %}
    select distinct schema_name
    from {{ information_schema_name(database) }}.SCHEMATA
    where catalog_name ilike '{{ database }}'
  {% endset %}
  {{ return(run_query(sql)) }}
{% endmacro %}


-- funcsign: (information_schema, string) -> agate_table
{% macro check_schema_exists(information_schema, schema) -%}
  {{ return(adapter.dispatch('check_schema_exists', 'dbt')(information_schema, schema)) }}
{% endmacro %}

-- funcsign: (information_schema, string) -> agate_table
{% macro default__check_schema_exists(information_schema, schema) -%}
  {% set sql -%}
        select count(*)
        from {{ information_schema.replace(information_schema_view='SCHEMATA') }}
        where catalog_name='{{ information_schema.database }}'
          and schema_name='{{ schema }}'
  {%- endset %}
  {{ return(run_query(sql)) }}
{% endmacro %}


-- funcsign: (string) -> agate_table
{% macro check_schema_exists_like(schema) -%}
  {{ return(adapter.dispatch('check_schema_exists_like', 'dbt')(schema)) }}
{% endmacro %}

-- funcsign: (string) -> agate_table
{% macro default__check_schema_exists_like(schema) -%}
  {{ exceptions.raise_not_implemented(
    'check_schema_exists_like macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}


-- funcsign: (relation) -> list[relation]
{% macro list_relations_without_caching(schema_relation) %}
  {{ return(adapter.dispatch('list_relations_without_caching', 'dbt')(schema_relation)) }}
{% endmacro %}

-- funcsign: (relation) -> list[relation]
{% macro default__list_relations_without_caching(schema_relation) %}
  {{ exceptions.raise_not_implemented(
    'list_relations_without_caching macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

-- funcsign: (relation) -> agate_table
{% macro get_catalog_for_single_relation(relation) %}
  {{ return(adapter.dispatch('get_catalog_for_single_relation', 'dbt')(relation)) }}
{% endmacro %}

-- funcsign: (relation) -> agate_table
{% macro default__get_catalog_for_single_relation(relation) %}
  {{ exceptions.raise_not_implemented(
    'get_catalog_for_single_relation macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

-- funcsign: () -> list[relation]
{% macro get_relations() %}
  {{ return(adapter.dispatch('get_relations', 'dbt')()) }}
{% endmacro %}

-- funcsign: () -> list[relation]
{% macro default__get_relations() %}
  {{ exceptions.raise_not_implemented(
    'get_relations macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

-- funcsign: (information_schema, list[relation]) -> agate_table
{% macro get_relation_last_modified(information_schema, relations) %}
  {{ return(adapter.dispatch('get_relation_last_modified', 'dbt')(information_schema, relations)) }}
{% endmacro %}

-- funcsign: (information_schema, list[relation]) -> agate_table
{% macro default__get_relation_last_modified(information_schema, relations) %}
  {{ exceptions.raise_not_implemented(
    'get_relation_last_modified macro not implemented for adapter ' + adapter.type()) }}
{% endmacro %}
