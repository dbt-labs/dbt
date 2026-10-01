{% macro databricks__dateadd(datepart, interval, from_date_or_timestamp) %}
  {%- if adapter.has_dbr_capability('timestampdiff') -%}
    timestampadd({{datepart}}, {{interval}}, {{from_date_or_timestamp}})
  {%- else -%}
    {{ spark__dateadd(datepart, interval, from_date_or_timestamp) }}
  {%- endif -%}
{%- endmacro %}
