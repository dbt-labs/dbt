{% set table_name = 'a' %}
{% set db_schema = t_schema_name() %}
{% set db_name = target.database %}

{% do set_up(db_schema, table_name) %}

{{ print(db_name) }}
{{ print(db_schema) }}

{% set relation = adapter.get_relation(
    database=db_name,
    schema=db_schema,
    identifier=table_name)
%}

{{ print("relation") }}
{{ print(relation) }}

{% set prefix = relation.get_ddl_prefix_for_create(undefined, temporary=true) %}
{{ print("prefix") }}
{{ print(prefix) }}

select 1 as one
