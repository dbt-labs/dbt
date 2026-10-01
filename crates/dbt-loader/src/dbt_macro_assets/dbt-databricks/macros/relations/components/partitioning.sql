{% macro get_create_sql_partition_by(partition_by) -%}
{%- if partition_by -%}
  PARTITIONED BY ({%- for col in partition_by -%}{{ col }}{% if not loop.last %}, {% endif %}{%- endfor %})
{%- endif -%}
{%- endmacro %}
