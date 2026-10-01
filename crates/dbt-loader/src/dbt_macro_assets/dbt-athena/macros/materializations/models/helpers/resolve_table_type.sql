{#
  Resolve the effective `table_type` ('hive' or 'iceberg') for the current model.

  Precedence:
    1. An explicit `table_type` model config always wins (backward compatible).
    2. Otherwise, the `table_format` of the catalog referenced via `catalog_name`
       (catalogs.yml v2). The default `glue` catalog yields 'iceberg'.
    3. Otherwise, default to 'hive' (the default `info_schema` catalog also
       resolves to 'hive', so unconfigured models are unchanged).
#}
{% macro resolve_table_type() %}
  {%- set catalog_relation = adapter.build_catalog_relation(config.model) -%}
  {%- set table_type = config.get('table_type', default=none) -%}
  {%- if table_type is none and catalog_relation is not none -%}
    {#- `table_format` renders Fusion's TableFormat, whose default is `default`,
        not `hive`; `table.sql` branches on the literal 'hive', so map it. -#}
    {%- set table_type = 'iceberg' if catalog_relation.table_format == 'iceberg' else 'hive' -%}
  {%- endif -%}
  {{ return((table_type or 'hive') | lower) }}
{% endmacro %}
