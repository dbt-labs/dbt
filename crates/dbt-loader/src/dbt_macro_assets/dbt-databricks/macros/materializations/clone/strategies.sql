{% macro databricks__create_or_replace_clone(this_relation, defer_relation) %}
    create or replace
    table {{ this_relation.render() }}
    shallow clone {{ defer_relation.render() }}
{% endmacro %}

{#- DIVERGENCE BEGIN: v2 does not store databricks_table_type, so we cannot
    check against it here. -#}
{% macro clone_requires_drop(existing_relation) %}
    {{ return(existing_relation is not none and not (existing_relation.is_table and existing_relation.is_shallow_clone)) }}
{% endmacro %}
{#- DIVERGENCE END -#}

{% macro create_or_replace_clone_external(this_relation, defer_relation) %}

    {%- set catalog_relation = adapter.build_catalog_relation(config.model) -%}

    create or replace
    table {{ this_relation.render() }}
    shallow clone {{ defer_relation.render() }}
    {{ location_clause(catalog_relation) }}

{% endmacro %}
