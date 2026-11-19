{% macro _log_relation(label, database, schema, identifier) %}
    {% set rel = adapter.get_relation(database=database, schema=schema, identifier=identifier) %}
    {% if rel is none %}
        {{ log(label ~ " [" ~ schema ~ "." ~ identifier ~ "] => None (not found)", info=True) }}
    {% else %}
        {{ log(label ~ " [" ~ schema ~ "." ~ identifier ~ "] => found, type=" ~ rel.type ~ ", relation=" ~ rel, info=True) }}
    {% endif %}
{% endmacro %}

{% macro check_relations() %}
    {% do _log_relation("table", target.database, target.schema, "table_hidden_schema") %}
    {% do _log_relation("view", target.database, target.schema, "view_hidden_schema") %}
    {% do _log_relation("materialized_view", target.database, target.schema, "materialized_view_hidden_schema") %}
{% endmacro %}
