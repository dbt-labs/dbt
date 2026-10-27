{#- Granular relation macros used by the shared create_backup /
    rename_intermediate / replace chains (e.g. when a model changes
    materialization type). The defaults raise compiler errors. -#}

{% macro mariadb__get_rename_table_sql(relation, new_name) -%}
    rename table {{ relation }} to {{ relation.incorporate(path={"identifier": new_name}) }}
{%- endmacro %}

{% macro mariadb__get_rename_view_sql(relation, new_name) -%}
    rename table {{ relation }} to {{ relation.incorporate(path={"identifier": new_name}) }}
{%- endmacro %}

{% macro mariadb__get_replace_table_sql(relation, sql) -%}
    {{ mariadb__create_table_as(False, relation, sql) }}
{%- endmacro %}

{% macro mariadb__get_replace_view_sql(relation, sql) -%}
    {{ mariadb__create_view_as(relation, sql) }}
{%- endmacro %}
