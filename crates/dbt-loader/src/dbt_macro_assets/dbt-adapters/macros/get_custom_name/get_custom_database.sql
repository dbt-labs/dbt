{#
    Renders a database name given a custom database name. If the custom
    database name is none, then the resulting database is just the "database"
    value in the specified target. If a database override is specified, then
    the custom database name is used instead of the default "database" value.

    This macro can be overriden in projects to define different semantics
    for rendering a database name.

    Arguments:
    custom_database_name: The custom database name specified for a model, or none
    node: The node the database is being generated for

#}

-- funcsign: (optional[string], optional[node]) -> string
{% macro generate_database_name(custom_database_name=none, node=none) -%}
    {% do return(adapter.dispatch('generate_database_name', 'dbt')(custom_database_name, node)) %}
{%- endmacro %}

-- funcsign: (optional[string], optional[node]) -> string
{% macro default__generate_database_name(custom_database_name=none, node=none) -%}
    {%- set default_database = target.database -%}
    {#- Only consult the catalog when the model actually references one. Adapters that
        don't support catalog integrations (e.g. redshift, postgres, spark) raise from
        build_catalog_relation, which would otherwise abort database resolution and drop
        the model's `database` config. -#}
    {%- set catalog_name = node.config.get('catalog_name') if (node is not none and node.config is defined) else none -%}
    {%- if catalog_name -%}
        {%- set catalog_relation = adapter.build_catalog_relation(node) -%}
        {%- if catalog_relation is not none and catalog_relation|attr('catalog_database') -%}
            {{ return(catalog_relation.catalog_database) }}
        {%- endif -%}
    {%- endif -%}
    {%- if custom_database_name is none -%}

        {{ default_database }}

    {%- else -%}

        {{ custom_database_name }}

    {%- endif -%}

{%- endmacro %}
