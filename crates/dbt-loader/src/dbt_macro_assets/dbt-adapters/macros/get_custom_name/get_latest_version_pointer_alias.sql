
{#
    Renders the alias for the latest-version pointer that dbt automatically creates
    for versioned models. By default the pointer takes the unsuffixed model name
    (e.g. "dim_customers"), which is the same name BI tools referenced before
    versioning was introduced.

    Follows the same pattern as generate_alias_name: if custom_alias_name is
    provided (via latest_version_pointer.alias config), it takes precedence, but
    the macro still has final say and can override it.

    Override this macro in your project to change the default behaviour. For
    example, to use a "_latest" suffix instead:

        {% macro generate_latest_version_pointer_alias(custom_alias_name=none, node=none) %}
            {{ node.name ~ "_latest" }}
        {% endmacro %}

    Arguments:
    custom_alias_name: The alias specified in latest_version_pointer.alias config, or none.
    node: The versioned ModelNode whose latest-version pointer alias is being generated.
#}

{% macro generate_latest_version_pointer_alias(custom_alias_name=none, node=none) -%}
    {% do return(adapter.dispatch('generate_latest_version_pointer_alias', 'dbt')(custom_alias_name, node)) %}
{%- endmacro %}

{% macro default__generate_latest_version_pointer_alias(custom_alias_name=none, node=none) -%}
    {%- if custom_alias_name -%}
        {{ custom_alias_name | trim }}
    {%- else -%}
        {{ node.name }}
    {%- endif -%}
{%- endmacro %}
