{% macro sqlserver__validate_sql(sql) -%}
  {% call statement('validate_sql') -%}
    {{ sql }}
  {% endcall %}
  {{ return(load_result('validate_sql')) }}
{% endmacro %}
