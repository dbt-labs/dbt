{%- if execute: -%}
  {% set parsed_partition_by = adapter.parse_partition_by(none) %}
{%- endif -%}

select 1 as calendar_date;