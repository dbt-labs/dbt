{#
    We expect a syntax error if dbt show is invoked both with a --limit flag to show
    and with a limit predicate embedded in its inline query. No special handling is
    provided out-of-box.
#}
-- funcsign: (string, string, optional[integer]) -> string
{% macro get_show_sql(compiled_code, sql_header, limit) -%}
  {%- if sql_header is not none -%}
  {{ sql_header }}
  {%- endif %}
  {{ get_limit_subquery_sql(compiled_code, limit) }}
{% endmacro %}

{#
    Not necessarily a true subquery anymore. Now, merely a query subordinate
    to the calling macro.
#}
-- funcsign: (string, optional[integer]) -> string
{%- macro get_limit_subquery_sql(sql, limit) -%}
  {{ adapter.dispatch('get_limit_sql', 'dbt')(sql, limit) }}
{%- endmacro -%}

-- funcsign: (string, optional[integer]) -> string
{% macro default__get_limit_sql(sql, limit) %}
  {{ sql }}
  {% if limit is not none %}
  limit {{ limit }}
  {%- endif -%}
{% endmacro %}
