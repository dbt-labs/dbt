{# DIVERGENCE: Fusion-only dispatch points so adapters can shape the clone target (dbt-labs/dbt-adapters#1767). #}
-- funcsign: (relation, relation) -> relation
{% macro get_clone_target_relation(this_relation, source_relation) %}
    {{ return(adapter.dispatch('get_clone_target_relation', 'dbt')(this_relation, source_relation)) }}
{% endmacro %}

-- funcsign: (relation, relation) -> relation
{% macro default__get_clone_target_relation(this_relation, source_relation) %}
    {{ return(this_relation.incorporate(type='table')) }}
{% endmacro %}

-- funcsign: (relation, relation) -> optional[string]
{% macro clone_target_drop_reason(existing_relation, target_relation) %}
    {{ return(adapter.dispatch('clone_target_drop_reason', 'dbt')(existing_relation, target_relation)) }}
{% endmacro %}

-- funcsign: (relation, relation) -> optional[string]
{% macro default__clone_target_drop_reason(existing_relation, target_relation) %}
    {% if not existing_relation.is_table %}
        {{ return("it is of type " ~ existing_relation.type) }}
    {% endif %}
    {{ return(none) }}
{% endmacro %}
