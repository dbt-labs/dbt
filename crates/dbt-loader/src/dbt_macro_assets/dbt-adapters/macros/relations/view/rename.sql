-- funcsign: (relation, string) -> string
{% macro get_rename_view_sql(relation, new_name) %}
    {{- adapter.dispatch('get_rename_view_sql', 'dbt')(relation, new_name) -}}
{% endmacro %}


-- funcsign: (relation, string) -> string
{% macro default__get_rename_view_sql(relation, new_name) %}
    {{ exceptions.raise_compiler_error(
        "`get_rename_view_sql` has not been implemented for this adapter."
    ) }}
{% endmacro %}
