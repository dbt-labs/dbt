{% macro file_format_clause(catalog_relation=none) %}
  {#--
    Moving forward, this macro should require a `catalog_relation`, which is covered by the first condition.
    However, there could be existing macros that is still passing no arguments, including user macros.
    Hence, we need to support the old code still, which is covered by the second condition.
  --#}
  {% if catalog_relation is not none %}
    {%- set table_format = catalog_relation.table_format -%}
    {%- set file_format = catalog_relation.file_format -%}
  {% else %}
    {%- set table_format = config.get('table_format', default='default') -%}
    {%- set file_format = adapter.resolve_file_format(config) -%}
  {% endif %}
  
  {#-- DIVERGENCE BEGIN: v2: managed iceberg is default; use_uniform=true opts into
       delta+tblprops. v1: use_managed_iceberg behavior flag drives the iceberg DDL form.
       `adapter.behavior.use_catalogs_v2` is a Fusion-only behavior flag; accessing it
       under dbt-core (1.x) raises "flag has not been registered". Gate on dbt_version. --#}
  {% if dbt_version.startswith('2.') and adapter.behavior.use_catalogs_v2.no_warn %}
    {% if table_format == 'iceberg' %}
      {% if catalog_relation is not none and catalog_relation.use_uniform %}
        using delta
      {% else %}
        using iceberg
      {% endif %}
    {% else %}
      using {{ file_format }}
    {% endif %}
  {% else %}
    {% if adapter.behavior.use_managed_iceberg and table_format == 'iceberg' %}
      using iceberg
    {% else %}
      using {{ file_format }}
    {% endif %}
  {% endif %}
  {#-- DIVERGENCE END --#}
{%- endmacro -%}


{% macro get_file_format(catalog_relation=none) %}
  {#-
    Moving forward, this macro should require a `catalog_relation`, which is covered by the first condition.
    However, there could be existing macros that is still passing no arguments, including user macros.
    Hence, we need to support the old code still, which is covered by the second condition.
  -#}
  {% if catalog_relation is not none %}
    {%- set raw_file_format = catalog_relation.file_format -%}
  {% else %}
    {%- set raw_file_format = adapter.resolve_file_format(config) -%}
  {% endif %}
  {% do return(dbt_databricks_validate_get_file_format(raw_file_format)) %}
{% endmacro %}
