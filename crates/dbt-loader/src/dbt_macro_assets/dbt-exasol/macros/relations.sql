{#- Granular relation macros used by the shared create_backup /
    rename_intermediate / replace chains (e.g. when a model changes
    materialization type). The defaults raise compiler errors. -#}

{% macro exasol__get_rename_table_sql(relation, new_name) -%}
    rename table {{ relation }} to {{ new_name }}
{%- endmacro %}

{% macro exasol__get_rename_view_sql(relation, new_name) -%}
    rename view {{ relation }} to {{ new_name }}
{%- endmacro %}

{% macro exasol__get_replace_table_sql(relation, sql) -%}
    {{ exasol__create_table_as(False, relation, sql) }}
{%- endmacro %}

{% macro exasol__get_replace_view_sql(relation, sql) -%}
    {{ exasol__create_view_as(relation, sql) }}
{%- endmacro %}
