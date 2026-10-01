{#
    This is identical to the implementation in dbt-core.
    We need to override because dbt-spark overrides to something we don't like.
#}

{# CORE DISCREPANCY: fallback for non catalogs.yml enabled models; TODO: investigate why nodes have different metadata in Fusion vs. Core #}
{% macro databricks__generate_database_name(custom_database_name=none, node=none) -%}
    {%- set default_database = target.database -%}
    {%- if node is not none and node|attr('database') -%}
        {%- set catalog_relation = adapter.build_catalog_relation(node) -%}
    {%- elif 'config' in target -%}
        {%- set catalog_relation = adapter.build_catalog_relation(target) -%}
    {%- else -%}
        {%- set catalog_relation = none -%}
    {%- endif -%}
    {%- if catalog_relation is not none
        and catalog_relation|attr('catalog_database') -%}
        {{ return(catalog_relation.catalog_database) }}
    {%- elif custom_database_name is none -%}
        {{ return((catalog_relation.catalog_name or default_database) if catalog_relation is not none else default_database) }}
    {%- else -%}
       {{ return(custom_database_name) }}
    {%- endif -%}
{%- endmacro %}
