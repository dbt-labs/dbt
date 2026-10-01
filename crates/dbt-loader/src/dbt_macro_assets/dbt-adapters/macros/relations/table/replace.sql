{% macro get_replace_table_sql(relation, sql) %}
    {{- adapter.dispatch('get_replace_table_sql', 'dbt')(relation, sql) -}}
{% endmacro %}


{% macro default__get_replace_table_sql(relation, sql) %}
    {{ exceptions.raise_compiler_error(
        "`get_replace_table_sql` has not been implemented for this adapter."
    ) }}
{% endmacro %}
