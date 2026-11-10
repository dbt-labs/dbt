{% macro exasol__create_schema(relation) -%}
  {%- call statement('create_schema') -%}
    create schema if not exists {{ relation.without_identifier().include(database=False) }}
  {%- endcall -%}
{% endmacro %}

{% macro exasol__drop_schema(relation) -%}
  {%- call statement('drop_schema') -%}
    drop schema if exists {{ relation.without_identifier().include(database=False) }} cascade
  {%- endcall -%}
{% endmacro %}

{% macro exasol__create_table_as(temporary, relation, sql) -%}
  {#- Exasol CTAS cannot carry inline column or constraint definitions, so
      contract constraints and the Exasol table-tuning configs are applied as
      separate post-CTAS ALTER statements (Fusion splits on ';'). -#}
  {%- set contract_config = config.get('contract') -%}
  {%- if contract_config.enforced -%}
    {{- get_assert_columns_equivalent(sql) }}
  {%- endif %}
  create or replace table {{ relation }} as (
    {%- if contract_config.enforced %}
    {{ get_select_subquery(sql) }}
    {%- else %}
    {{ sql }}
    {%- endif %}
  )
  {{- exasol__post_create_table_alters(relation, contract_config) }}
{%- endmacro %}

{% macro exasol__create_view_as(relation, sql) -%}
  {#- Exasol view/column comments can only be set inside CREATE VIEW
      (COMMENT ON VIEW is rejected), so persist_docs for views lives in the DDL. -#}
  {%- set contract_config = config.get('contract') -%}
  {%- if contract_config.enforced -%}
    {{- get_assert_columns_equivalent(sql) }}
  {%- endif %}
  {%- set col_docs = {} -%}
  {%- if config.persist_column_docs() and model.columns -%}
    {%- for col_name, col in model.columns.items() -%}
      {%- if col.description -%}
        {%- do col_docs.update({col_name | lower: col.description}) -%}
      {%- endif -%}
    {%- endfor -%}
  {%- endif -%}
  create or replace view {{ relation }}
  {%- if col_docs %}
  (
    {%- set query_columns = get_columns_in_query(sql) %}
    {%- for column_name in query_columns %}
    {{ adapter.quote(column_name) }}{% if col_docs.get(column_name | lower) %} comment is '{{ col_docs[column_name | lower] | replace("'", "''") }}'{% endif %}{{ ',' if not loop.last }}
    {%- endfor %}
  )
  {%- endif %}
  as (
    {{ sql }}
  )
  {%- if config.persist_relation_docs() and model.description %}
  comment is '{{ model.description | replace("'", "''") }}'
  {%- endif %}
{%- endmacro %}

{% macro exasol__drop_relation(relation) -%}
  {% call statement('drop_relation', auto_begin=False) -%}
    drop {{ relation.type }} if exists {{ relation }} cascade
  {%- endcall %}
{% endmacro %}

{% macro exasol__rename_relation(from_relation, to_relation) -%}
  {#- Quote the target per the quoting policy. Exasol uppercases unquoted
      identifiers, so a bare target name would break refs that render the
      relation quoted (the default quoting policy). -#}
  {% call statement('rename_relation') -%}
    rename {{ from_relation.type }} {{ from_relation }} to {{ adapter.quote_as_configured(to_relation.identifier, 'identifier') }}
  {%- endcall %}
{% endmacro %}

{% macro exasol__truncate_relation(relation) -%}
  {% call statement('truncate_relation') -%}
    truncate table {{ relation }}
  {%- endcall %}
{% endmacro %}

{% macro exasol__list_schemas(database) -%}
  {% call statement('list_schemas', fetch_result=True, auto_begin=False) -%}
    select schema_name as "name" from sys.exa_schemas
  {%- endcall %}
  {{ return(load_result('list_schemas').table) }}
{% endmacro %}

{% macro exasol__check_schema_exists(information_schema, schema) -%}
  {% call statement('check_schema_exists', fetch_result=True, auto_begin=False) -%}
    select count(*) from sys.exa_schemas where upper(schema_name) = upper('{{ schema }}')
  {%- endcall %}
  {{ return(load_result('check_schema_exists').table) }}
{% endmacro %}

{% macro exasol__information_schema_name(database) -%}
  sys
{%- endmacro %}

{% macro exasol__current_timestamp() -%}
  current_timestamp
{%- endmacro %}

{% macro exasol__get_columns_in_relation(relation) -%}
  {% call statement('get_columns_in_relation', fetch_result=True) %}
    select
      column_name,
      column_type as data_type,
      column_maxsize as character_maximum_length,
      column_num_prec as numeric_precision,
      column_num_scale as numeric_scale
    from sys.exa_all_columns
    where upper(column_table) = upper('{{ relation.identifier }}')
      {% if relation.schema %}
      and upper(column_schema) = upper('{{ relation.schema }}')
      {% endif %}
    order by column_ordinal_position
  {% endcall %}
  {% set table = load_result('get_columns_in_relation').table %}
  {{ return(sql_convert_columns_in_relation(table)) }}
{%- endmacro %}

{% macro exasol__list_relations_without_caching(schema_relation) -%}
  {% call statement('list_relations_without_caching', fetch_result=True) -%}
    select
      '{{ schema_relation.database }}' as "database",
      table_name as "name",
      table_schema as "schema",
      'table' as "type"
    from sys.exa_all_tables
    where upper(table_schema) = upper('{{ schema_relation.schema }}')
    union all
    select
      '{{ schema_relation.database }}' as "database",
      view_name as "name",
      view_schema as "schema",
      'view' as "type"
    from sys.exa_all_views
    where upper(view_schema) = upper('{{ schema_relation.schema }}')
  {%- endcall %}
  {{ return(load_result('list_relations_without_caching').table) }}
{%- endmacro %}
