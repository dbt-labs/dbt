-- Exercise bigquery__get_show_grant_sql macro path
{{ setup_test_table() }}

{% set relation = adapter.get_relation(database=target.database, schema=target.schema, identifier='info_schema_test_table') %}
{% if relation %}
  -- macro does incorporate(location=...) before OBJECT_PRIVILEGES
  {% set sql = get_show_grant_sql(relation) %}
  {{ log("relation: " ~ relation, info=true) }}
  {{ log("relation.render(): " ~ relation.render(), info=true) }}
  {{ log("SHOW GRANTS SQL: " ~ sql, info=true) }}
  {{ sql }}
{% else %}
  select 'SKIPPED - relation not found' as result
{% endif %}


