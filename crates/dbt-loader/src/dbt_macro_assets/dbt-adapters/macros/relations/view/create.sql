-- funcsign: (relation, string) -> string
{% macro get_create_view_as_sql(relation, sql) -%}
  {{ adapter.dispatch('get_create_view_as_sql', 'dbt')(relation, sql) }}
{%- endmacro %}

-- funcsign: (relation, string) -> string
{% macro default__get_create_view_as_sql(relation, sql) -%}
  {{ return(create_view_as(relation, sql)) }}
{% endmacro %}


/* {# keep logic under old name for backwards compatibility #} */
-- funcsign: (relation, string) -> string
{% macro create_view_as(relation, sql) -%}
  {{ adapter.dispatch('create_view_as', 'dbt')(relation, sql) }}
{%- endmacro %}

-- funcsign: (relation, string) -> string
{% macro default__create_view_as(relation, sql) -%}
  {%- set sql_header = config.get('sql_header', none) -%}

  {{ sql_header if sql_header is not none }}
  create view {{ relation.render() }}
    {% set contract_config = config.get('contract') %}
    {% if contract_config.enforced %}
      {{ get_assert_columns_equivalent(sql) }}
    {%- endif %}
  as (
    {{ sql }}
  );
{%- endmacro %}
