-- funcsign: (bool, relation, string) -> string
{% macro get_create_table_as_sql(temporary, relation, sql) -%}
  {{ adapter.dispatch('get_create_table_as_sql', 'dbt')(temporary, relation, sql) }}
{%- endmacro %}

-- funcsign: (bool, relation, string) -> string
{% macro default__get_create_table_as_sql(temporary, relation, sql) -%}
  {{ return(create_table_as(temporary, relation, sql)) }}
{% endmacro %}


/* {# keep logic under old macro name for backwards compatibility #} */
-- funcsign: (bool, relation, string, optional[string]) -> string
{% macro create_table_as(temporary, relation, compiled_code, language='sql') -%}
  {# backward compatibility for create_table_as that does not support language #}
  {% if language == "sql" %}
    {{ adapter.dispatch('create_table_as', 'dbt')(temporary, relation, compiled_code)}}
  {% else %}
    {{ adapter.dispatch('create_table_as', 'dbt')(temporary, relation, compiled_code, language) }}
  {% endif %}

{%- endmacro %}

-- funcsign: (bool, relation, string) -> string
{% macro default__create_table_as(temporary, relation, sql) -%}
  {%- set sql_header = config.get('sql_header', none) -%}

  {{ sql_header if sql_header is not none }}

  create {% if temporary: -%}temporary{%- endif %} table
    {{ relation.include(database=(not temporary), schema=(not temporary)) }}
  {% set contract_config = config.get('contract') %}
  {% if contract_config.enforced and (not temporary) %}
    {{ get_assert_columns_equivalent(sql) }}
    {{ get_table_columns_and_constraints() }}
    {%- set sql = get_select_subquery(sql) %}
  {% endif %}
  as (
    {{ sql }}
  );
{%- endmacro %}


-- funcsign: () -> string
{% macro default__get_column_names() %}
  {#- loop through user_provided_columns to get column names -#}
    {%- set user_provided_columns = model['columns'] -%}
    {%- for i in user_provided_columns %}
      {%- set col = user_provided_columns[i] -%}
      {%- set col_name = adapter.quote(col['name']) if col.get('quote') else col['name'] -%}
      {{ col_name }}{{ ", " if not loop.last }}
    {%- endfor -%}
{% endmacro %}


-- funcsign: (string) -> string
{% macro get_select_subquery(sql) %}
  {#-- DIVERGENCE BEGIN: strip a terminal `;` so it does not land inside the
       wrapper built below; done at the dispatcher so the adapter overrides are
       covered too. See `StmtSplitter::strip_trailing_statement_terminator`.
       Upstream: https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-adapters/src/dbt/include/global_project/macros/relations/table/create.sql --#}
  {%- if dbt_version.startswith('2.') -%}
    {%- set sql = adapter.strip_trailing_statement_terminator(sql) -%}
  {%- endif -%}
  {#-- DIVERGENCE END --#}
  {{ return(adapter.dispatch('get_select_subquery', 'dbt')(sql)) }}
{% endmacro %}

-- funcsign: (string) -> string
{% macro default__get_select_subquery(sql) %}
    select {{ adapter.dispatch('get_column_names', 'dbt')() }}
    from (
        {{ sql }}
    ) as model_subq
{%- endmacro %}
