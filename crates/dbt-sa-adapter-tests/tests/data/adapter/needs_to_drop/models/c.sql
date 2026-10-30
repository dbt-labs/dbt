{% set table_name = 'a' %}
{% set db_schema = t_schema_name() %}
{% set db_name = target.database %}

{% do set_up(db_schema, table_name) %}

{{ print(db_name) }}
{{ print(db_schema) }}

{% set relation = adapter.get_relation(
      database=db_name,
      schema=db_schema,
      identifier='a')
%}

{% if relation %}
  {{ print("something") }}
{% endif %}

{{ print(relation) }}

{% set val = relation.needs_to_drop(relation) %}

{{ print(val) }}

SELECT 1 as one
