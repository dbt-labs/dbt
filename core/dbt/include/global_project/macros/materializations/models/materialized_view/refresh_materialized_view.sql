{% macro refresh_materialized_view(relation) %}
    {{- log('Applying REFRESH to: ' ~ relation) -}}
    {{- adapter.dispatch('refresh_materialized_view', 'dbt')(relation) -}}
{% endmacro %}


{% macro default__refresh_materialized_view(relation) %}
    {{ exceptions.raise_compiler_error("Materialized views have not been implemented for this adapter.") }}
{% endmacro %}
