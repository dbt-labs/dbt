{% set db_name = target.database  %}
{% set db_schema = t_schema_name() %}
{% set table_name = 'a' %}

{% do set_up(db_schema, table_name) %}

-- `dbt run` fails without this config
-- create this model in another schema since public schema gonna be dropped
{{ config(schema=db_schema ~ '_bak') }}

{%
  set relation = adapter.get_relation(
      database=db_name,
      schema=db_schema,
      identifier=table_name)
%}
{{ print("relation before drop_schema") }}
{{ print(relation) }}

{% set result = adapter.drop_schema(relation) %}
{{ print("drop_schema result") }}
{{ print(result) }}

SELECT 1
