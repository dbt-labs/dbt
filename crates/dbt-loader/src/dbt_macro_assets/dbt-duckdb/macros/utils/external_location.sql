{%- macro external_location(relation, config) -%}
  {%- set catalog_relation = adapter.build_catalog_relation(model) -%}
  {%- set external_root = adapter.external_root() -%}
  {%- if catalog_relation is not none and catalog_relation|attr('external_root') -%}
    {%- set external_root = catalog_relation.external_root -%}
  {%- endif -%}
  {%- if config.get('options', {}).get('partition_by') is none -%}
    {%- set format = config.get('format', none) -%}
    {%- if format is none and catalog_relation is not none and catalog_relation|attr('file_format') -%}
      {%- set format = catalog_relation.file_format -%}
    {%- endif -%}
    {%- if format is none -%}
      {%- set format = 'parquet' -%}
    {%- endif -%}
    {{- external_root }}/{{ relation.identifier }}.{{ format }}
  {%- else -%}
    {{- external_root }}/{{ relation.identifier }}
  {%- endif -%}
{%- endmacro -%}
