{% set table_name = 'a' %}
{% set db_schema = t_schema_name() %}
{% set db_name = target.database %}

{% do set_up(db_schema, table_name) %}

{{ print(db_name) }}
{{ print(db_schema) }}

{# adapter.get_relation triggers list_relations on cache miss; with
   datasharing: true the Rust path emits `SHOW TABLES FROM SCHEMA <db>.<schema>`
   instead of the default information_schema query. #}
{%
set relation = adapter.get_relation(
    database=db_name,
    schema=db_schema,
    identifier=table_name)
%}

{{ print("relation") }}
{{ print(relation) }}

SELECT 1 AS one
