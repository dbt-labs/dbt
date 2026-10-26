{% set table_name = 'a' %}
{% set db_schema = t_schema_name() %}
{% set db_name = target.database %}

{% do set_up(db_schema, table_name) %}

{{ print(db_name) }}
{{ print(db_schema) }}

{% set relation = load_relation( ref('a') ) %}
{{ print(relation) }}

{{ print(relation.include(database=true)) }}
{{ print(relation.include(database=true, schema=true)) }}
{{ print(relation.include(database=true, schema=true, identifier=true)) }}
{{ print(this.MaterializedView) }}
{{ print(this.Table) }}
select 1 as id
