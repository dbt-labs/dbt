-- funcsign: (optional[string], optional[node]) -> string
{% macro bigquery__generate_schema_name(custom_schema_name=none, node=none) -%}
    {%- set namespace = default__generate_schema_name(custom_schema_name, node) -%}
    {%- set catalog_name = node.config.get('catalog_name') if (node is not none and node.config is defined) else none -%}
    {%- if catalog_name -%}
        {%- set catalog_relation = adapter.build_catalog_relation(node) -%}
    {%- else -%}
        {%- set catalog_relation = none -%}
    {%- endif -%}
    {%- if catalog_relation is not none and catalog_relation|attr('lakehouse_catalog') -%}
        {{ return(catalog_relation.lakehouse_catalog ~ '.' ~ (namespace | trim)) }}
    {%- else -%}
        {{ return(namespace) }}
    {%- endif -%}
{%- endmacro %}
