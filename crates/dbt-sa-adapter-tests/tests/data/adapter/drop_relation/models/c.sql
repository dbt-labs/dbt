{% set db_name = target.database %}
{% set db_schema = t_schema_name() %}
{% set table_name = 'a' %}

{% do set_up(db_schema, table_name) %}

{%
  set relation = adapter.get_relation(
       database=db_name,
       schema=db_schema,
       identifier=table_name)
%}

{{ print(relation) }}

{% set _ = adapter.drop_relation(relation) %}

-- Comment to ensure that c runs after a {{ ref('a') }}

SELECT one FROM {{ relation }}
