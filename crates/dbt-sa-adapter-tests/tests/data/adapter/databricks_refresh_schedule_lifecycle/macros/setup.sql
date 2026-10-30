{% macro set_up() %}
  {% set schema_relation = api.Relation.create(database=target.database, schema=target.schema) %}
  {% do adapter.create_schema(schema_relation) %}
  {% call statement('create_refresh_source') %}
    CREATE TABLE {{ source('refresh_input', 'refresh_source') }}
    USING DELTA
    TBLPROPERTIES ('delta.enableRowTracking' = 'true')
    AS SELECT CAST(1 AS INT) AS id
  {% endcall %}
{% endmacro %}
