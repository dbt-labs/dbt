{% macro bigquery__refresh_materialized_view(relation) %}
    call bq.refresh_materialized_view('{{ relation.database }}.{{ relation.schema }}.{{ relation.identifier }}')
{% endmacro %}
