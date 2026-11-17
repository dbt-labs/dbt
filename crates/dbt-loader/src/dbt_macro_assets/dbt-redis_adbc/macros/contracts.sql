{#- CREATE TABLE … AS, with model contracts. dbt's default writes a column
    list *and* AS SELECT, which neither Postgres nor the driver accepts; as
    on dbt-postgres, an enforced contract creates the table from its DDL
    (columns, types, constraints) and then inserts the rows. -#}
{% macro redis_adbc__create_table_as(temporary, relation, compiled_code, language='sql') -%}
  {%- if language != 'sql' -%}
    {{ exceptions.raise_compiler_error("redis_adbc's create_table_as builds SQL models; its table materialization builds Python models") }}
  {%- endif -%}
  {%- set sql_header = config.get('sql_header', none) -%}
  {{ sql_header if sql_header is not none }}
  {%- set contract_config = config.get('contract') %}
  create {% if temporary -%}temporary {% endif -%}table {{ relation }}
  {%- if contract_config.enforced %}
    {{ get_assert_columns_equivalent(compiled_code) }}
  {%- endif %}
  {%- if contract_config.enforced and not temporary %}
    {{ get_table_columns_and_constraints() }};
  insert into {{ relation }} ({{ adapter.dispatch('get_column_names', 'dbt')() }})
    {%- set compiled_code = get_select_subquery(compiled_code) %}
  {%- else %}
  as
  {%- endif %}
  (
    {{ compiled_code }}
  );
{%- endmacro %}
