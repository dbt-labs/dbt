{% macro sqlserver__get_create_index_sql(relation, index_dict) -%}
  {{ exceptions.raise_compiler_error(
    "Custom `indexes:` config is not implemented yet in dbt Core v2's SQL Server "
    "adapter (tracked as a follow-up issue). Remove the indexes config from "
    ~ relation ~ " to build without it."
  ) }}
{%- endmacro %}
