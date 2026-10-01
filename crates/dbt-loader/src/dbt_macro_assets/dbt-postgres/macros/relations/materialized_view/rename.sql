{% macro postgres__get_rename_materialized_view_sql(relation, new_name) %}
    alter materialized view {{ relation }} rename to {{ new_name }}
{% endmacro %}
