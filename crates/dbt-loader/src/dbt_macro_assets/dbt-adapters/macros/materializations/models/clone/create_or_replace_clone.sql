-- funcsign: (relation, relation) -> string
{% macro create_or_replace_clone(this_relation, defer_relation) %}
    {{ return(adapter.dispatch('create_or_replace_clone', 'dbt')(this_relation, defer_relation)) }}
{% endmacro %}

-- funcsign: (relation, relation) -> string
{% macro default__create_or_replace_clone(this_relation, defer_relation) %}
    create or replace table {{ this_relation.render() }} clone {{ defer_relation.render() }}
{% endmacro %}
