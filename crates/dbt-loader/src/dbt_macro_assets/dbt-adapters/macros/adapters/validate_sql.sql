-- funcsign: (string) -> agate_table
{% macro validate_sql(sql) -%}
  {{ return(adapter.dispatch('validate_sql', 'dbt')(sql)) }}
{% endmacro %}

-- funcsign: (string) -> agate_table
{% macro default__validate_sql(sql) -%}
  {% call statement('validate_sql') -%}
    explain {{ sql }}
  {% endcall %}
  {{ return(load_result('validate_sql')) }}
{% endmacro %}
