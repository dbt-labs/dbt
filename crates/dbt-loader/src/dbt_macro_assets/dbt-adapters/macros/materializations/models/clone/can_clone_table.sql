-- funcsign: () -> bool
{% macro can_clone_table() %}
    {{ return(adapter.dispatch('can_clone_table', 'dbt')()) }}
{% endmacro %}

-- funcsign: () -> bool
{% macro default__can_clone_table() %}
    {{ return(False) }}
{% endmacro %}
