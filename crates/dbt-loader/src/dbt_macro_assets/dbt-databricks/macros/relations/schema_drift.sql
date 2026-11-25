{#-- CREATE embeds an explicit column list (Databricks user-specified schema).
    When that drifts from the current query (e.g. upstream `select *` column add),
    REFRESH fails — the caller replaces instead (#1359). The model's query text is
    unchanged in that scenario, so the QueryProcessor component of
    get_configuration_changes cannot detect it; callers treat the result as an
    additional configuration change. Names only: type-label variance across DESCRIBE
    paths would cause spurious recreates. Streaming tables (#1303) are out of scope
    here: locking columns on CREATE is a separate design question. --#}
{% macro dlt_inferred_query_schema_changed(relation, sql) -%}
  {%- set inferred_names = get_columns_in_query(sql) | map('lower') | list -%}
  {%- set existing_names = adapter.get_columns_in_relation(relation)
                             | map(attribute='name') | map('lower') | list -%}
  {%- set drifted = inferred_names != existing_names -%}
  {%- if drifted -%}
    {%- do log("Materialized view " ~ relation ~ " query schema drifted (was " ~ existing_names
               ~ ", now " ~ inferred_names ~ "); recreating instead of refreshing.") -%}
  {%- endif -%}
  {%- do return(drifted) -%}
{%- endmacro %}
