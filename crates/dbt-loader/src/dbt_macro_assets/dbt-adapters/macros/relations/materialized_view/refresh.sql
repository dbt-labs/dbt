-- funcsign: (relation) -> string
{% macro refresh_materialized_view(relation) %}
    {{- log('Applying REFRESH to: ' ~ relation) -}}
    {{- adapter.dispatch('refresh_materialized_view', 'dbt')(relation) -}}
{% endmacro %}


-- funcsign: (relation) -> string
{% macro default__refresh_materialized_view(relation) %}
    {{ exceptions.raise_compiler_error("`refresh_materialized_view` has not been implemented for this adapter.") }}
{% endmacro %}
