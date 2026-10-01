
{% macro redshift__snapshot_merge_sql(target, source, insert_cols) -%}
    {{ postgres__snapshot_merge_sql(target, source, insert_cols) }}
{% endmacro %}
