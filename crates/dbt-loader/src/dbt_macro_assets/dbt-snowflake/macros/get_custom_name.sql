{% macro snowflake__generate_database_name(custom_database_name=none, node=none) -%}
    {# -- adapter in Core is Snowflake, here it's the ParseAdapter #}
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
        {%- if catalog_relation is not none
            and catalog_relation|attr('catalog_linked_database') -%}
            {{ return(catalog_relation.catalog_linked_database) }}
        {%- else -%}
            {{ target.database }}
        {%- endif -%}
    {%- else -%}
       {{ custom_database_name }}
    {%- endif -%}
{%- endmacro %}


{# DIVERGENCE: fusion does not have a snowflake__generate_schema_name macro, below is commented out on purpose, check if we need it for CLD implementations #}
{#
{% macro snowflake__generate_schema_name(custom_schema_name, node) -%}

    {%- set default_schema = target.schema -%}
    {%- if custom_schema_name is not none -%}

        {%- set default_schema =  default_schema ~ "_" ~ custom_schema_name.strip() -%}

    {%- endif -%}

    {%- if node is not none and node|attr('database') -%}

            {%- set catalog_relation = adapter.build_catalog_relation(node) -%}

            {%- if catalog_relation.catalog_linked_database_type is defined and
                catalog_relation.catalog_linked_database_type == 'glue' -%}
                {%- set default_schema = default_schema.lower() -%}
            {%- endif -%}
    {%- endif -%}

    {{ default_schema }}

{%- endmacro %}
#}
