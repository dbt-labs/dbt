
{#
    Renders a schema name given a custom schema name. If the custom
    schema name is none, then the resulting schema is just the "schema"
    value in the specified target. If a schema override is specified, then
    the resulting schema is the default schema concatenated with the
    custom schema.

    This macro can be overriden in projects to define different semantics
    for rendering a schema name.

    Arguments:
    custom_schema_name: The custom schema name specified for a model, or none
    node: The node the schema is being generated for

#}
-- funcsign: (optional[string], optional[node]) -> string
{% macro generate_schema_name(custom_schema_name=none, node=none) -%}
    {{ return(adapter.dispatch('generate_schema_name', 'dbt')(custom_schema_name, node)) }}
{% endmacro %}

-- funcsign: (optional[string], optional[node]) -> string
{% macro default__generate_schema_name(custom_schema_name, node) -%}

    {%- set default_schema = target.schema -%}
    {%- if custom_schema_name is none -%}

        {{ default_schema }}

    {%- else -%}

        {{ default_schema }}_{{ custom_schema_name | trim }}

    {%- endif -%}

{%- endmacro %}


{#
    Renders a schema name given a custom schema name. In production, this macro
    will render out the overriden schema name for a model. Otherwise, the default
    schema specified in the active target is used.

    Arguments:
    custom_schema_name: The custom schema name specified for a model, or none
    node: The node the schema is being generated for

#}
-- funcsign: (optional[string], optional[node]) -> string
{% macro generate_schema_name_for_env(custom_schema_name, node) -%}

    {%- set default_schema = target.schema -%}
    {%- if target.name == 'prod' and custom_schema_name is not none -%}

        {{ custom_schema_name | trim }}

    {%- else -%}

        {{ default_schema }}

    {%- endif -%}

{%- endmacro %}
