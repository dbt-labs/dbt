{% macro get_replace_materialized_view_as_sql(relation, sql, existing_relation, backup_relation, intermediate_relation) %}
    {{- log('Applying REPLACE to: ' ~ relation) -}}
    {{- adapter.dispatch('get_replace_materialized_view_as_sql', 'dbt')(relation, sql, existing_relation, backup_relation, intermediate_relation) -}}
{% endmacro %}


{% macro default__get_replace_materialized_view_as_sql(relation, sql, existing_relation, backup_relation, intermediate_relation) %}
    {{ exceptions.raise_compiler_error("Materialized views have not been implemented for this adapter.") }}
{% endmacro %}
