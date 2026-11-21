{% set db_name = target.database %}
{% set db_schema = t_schema_name() %}
{% set table_name = 'a' %}

{% do set_up(db_schema, table_name) %}

{% set result = adapter.check_schema_exists(database=db_name, schema=db_schema) %}
{{ print(result ~ ": target.schema exists") }}

{% set result = adapter.check_schema_exists(database=db_name, schema="nonexistent_schema") %}
{{ print(result ~ ": nonexistent_schema exists") }}

select * from {{ ref("a") }}
