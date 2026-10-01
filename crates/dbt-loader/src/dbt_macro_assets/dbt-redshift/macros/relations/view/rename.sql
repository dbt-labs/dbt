{% macro redshift__get_rename_view_sql(relation, new_name) %}
    alter table {{ relation }} rename to {{ new_name }}
{% endmacro %}
