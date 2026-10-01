
{# TODO: delete this file in favor of get_latest_version_pointer_alias.sql, or apply the same file name in v1 macro #}
{#
    Renders the alias for the latest version pointer view.

    If a custom alias is specified in the model's `latest_version_pointer.alias`
    config, it is passed as `custom_alias_name`. Otherwise, `custom_alias_name`
    is none and the default behavior returns the unsuffixed model name.

    Override this macro in your project to customize the pointer view name
    globally (e.g., appending `_latest` to all pointer views).

    Arguments:
    custom_alias_name: The custom alias from `latest_version_pointer.alias`, or none
    node: The model node that a pointer is being generated for

#}

-- funcsign: (optional[string], optional[node]) -> string
{% macro generate_latest_version_pointer_alias(custom_alias_name=none, node=none) -%}
    {% do return(adapter.dispatch('generate_latest_version_pointer_alias', 'dbt')(custom_alias_name, node)) %}
{%- endmacro %}

-- funcsign: (optional[string], optional[node]) -> string
{% macro default__generate_latest_version_pointer_alias(custom_alias_name=none, node=none) -%}

    {%- if custom_alias_name -%}

        {{ custom_alias_name | trim }}

    {%- else -%}

        {{ node.name }}

    {%- endif -%}

{%- endmacro %}
