{% set db_name = target.database %}
{% set db_schema = t_schema_name() %}
{% set table_name = 'a' %}
{% set new_table_name = 'b' %}

{% do set_up(db_schema, table_name) %}

-- not existed yet
{%
set to_relation = adapter.get_relation(
    database=db_name,
    schema=db_schema,
    identifier=new_table_name)
%}
{{ print("to_relation") }}
{{ print(to_relation) }}

{%
set from_relation = adapter.get_relation(
    database=db_name,
    schema=db_schema,
    identifier=table_name)
%}
{{ print("from_relation") }}
{{ print(from_relation) }}

{% set to_relation = from_relation.incorporate(path={"identifier": new_table_name}) %}
{% set result = adapter.rename_relation(from_relation, to_relation) %}
{{ print(result) }}

-- it exists now
{%
set renamed_relation = adapter.get_relation(
    database=db_name,
    schema=db_schema,
    identifier=new_table_name)
%}
{{ print("renamed_relation") }}
{{ print(renamed_relation) }}

select 1 as id
