
{#
    Renders a alias name given a custom alias name. If the custom
    alias name is none, then the resulting alias is just the filename of the
    model. If an alias override is specified, then that is used.

    This macro can be overriden in projects to define different semantics
    for rendering a alias name.

    Arguments:
    custom_alias_name: The custom alias name specified for a model, or none
    node: The available node that an alias is being generated for, or none

#}

-- funcsign: (optional[string], optional[node]) -> string
{% macro generate_alias_name(custom_alias_name=none, node=none) -%}
    {% do return(adapter.dispatch('generate_alias_name', 'dbt')(custom_alias_name, node)) %}
{%- endmacro %}

-- funcsign: (optional[string], optional[node]) -> string
{% macro default__generate_alias_name(custom_alias_name=none, node=none) -%}

    {%- if custom_alias_name -%}

        {{ custom_alias_name | trim }}

    {%- elif node.version -%}

        {{ return(node.name ~ "_v" ~ (node.version | replace(".", "_"))) }}

    {%- else -%}

        {{ node.name }}

    {%- endif -%}

{%- endmacro %}
