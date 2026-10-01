{% macro postgres__describe_materialized_view(relation) %}
    -- for now just get the indexes, we don't need the name or the query yet
    {% set _indexes = run_query(get_show_indexes_sql(relation)) %}
    {% do return({'indexes': _indexes}) %}
{% endmacro %}
