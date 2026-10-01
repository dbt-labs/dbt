{% macro databricks__datediff(first_date, second_date, datepart) %}
  {%- if adapter.has_dbr_capability('timestampdiff') -%}
    timestampdiff({{datepart}}, {{date_trunc(datepart, first_date)}}, {{date_trunc(datepart, second_date)}})
  {%- else -%}
    {{ spark__datediff(first_date, second_date, datepart) }}
  {%- endif -%}
{%- endmacro %}
