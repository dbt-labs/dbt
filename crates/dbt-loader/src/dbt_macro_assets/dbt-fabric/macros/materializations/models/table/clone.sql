{% macro fabric__can_clone_table() %}
    {{ return(True) }}
{% endmacro %}

{% macro fabric__create_or_replace_clone(target_relation, defer_relation) %}
    CREATE TABLE {{target_relation}}
    AS CLONE OF {{defer_relation}}
{% endmacro %}


{%- materialization clone, adapter='fabric' -%}

    {%- set relations = {'relations': []} -%}
    {%- if not defer_relation -%}
        -- nothing to do
        {%- if model.config.materialized == 'ephemeral' -%}
            {{ log("Skipping clone for ephemeral model " ~ model.unique_id, info=True) }}
        {%- else -%}
            {{ log("No relation found in state manifest for " ~ model.unique_id, info=True) }}
        {%- endif -%}
        {{ return(relations) }}
    {%- endif -%}

    {%- set other_existing_relation = load_cached_relation(defer_relation) -%}
    {% set can_clone_table = can_clone_table() %}

    {%- if other_existing_relation and other_existing_relation.type == 'table' and can_clone_table -%}
        {%- set target_relation = this.incorporate(type='table') -%}

        {% call statement('main') %}
            {% do adapter.drop_relation(target_relation) %}
            {{ create_or_replace_clone(target_relation, defer_relation) }}
        {% endcall %}
        {{ return({'relations': [target_relation]}) }}
    {%- else -%}

        {%- set target_relation = this.incorporate(type='view') -%}

        -- reuse the view materialization
        -- TODO: support actual dispatch for materialization macros
        -- Tracking ticket: https://github.com/dbt-labs/dbt-core/issues/7799
        {% set search_name = "materialization_view_" ~ adapter.type() %}
        {% if not search_name in context %}
            {% set search_name = "materialization_view_default" %}
        {% endif %}
        {% set materialization_macro = context[search_name] %}
        {% set relations = materialization_macro() %}
        {{ return(relations) }}
    {%- endif -%}


{%- endmaterialization -%}
