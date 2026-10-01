{#
    DuckDB override for generate_database_name.

    When a v2 catalog with a duckdb config block is in use, routes models
    to the ATTACH alias (attached_database) rather than target.database.
    Falls back to the default database for plain DuckDB / MotherDuck
    projects that do not use catalogs v2.
#}

-- funcsign: (optional[string], optional[node]) -> string
{% macro duckdb__generate_database_name(custom_database_name=none, node=none) -%}
    {%- set default_database = target.database -%}
    {%- if custom_database_name is none -%}
        {%- if node is not none and node|attr('database') -%}
            {%- set catalog_relation = adapter.build_catalog_relation(node) -%}
        {%- elif 'config' in target -%}
            {%- set catalog_relation = adapter.build_catalog_relation(target) -%}
        {%- else -%}
            {%- set catalog_relation = none -%}
        {%- endif -%}
        {%- if catalog_relation is not none
            and catalog_relation|attr('attached_database') -%}
            {{ return(catalog_relation.attached_database) }}
        {%- else -%}
            {{ return(default_database) }}
        {%- endif -%}
    {%- else -%}
        {{ return(custom_database_name) }}
    {%- endif -%}
{%- endmacro %}
