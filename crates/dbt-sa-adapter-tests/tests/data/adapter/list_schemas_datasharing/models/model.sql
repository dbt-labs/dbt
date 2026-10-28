{% set db_schema = t_schema_name() %}
{% set db_name = target.database %}

{% do set_up(db_schema, 'a') %}

{{ print(db_name) }}
{{ print(db_schema) }}

{% set schemas = adapter.list_schemas(db_name) %}

{# Exercise the pre-quoted database path used by dbt-core's create_schemas
   (which passes str(relation), already wrapped in double quotes). The
   redshift macro must NOT re-quote — both calls should emit identical
   `SHOW SCHEMAS FROM DATABASE "<db>"` SQL. #}
{% do adapter.list_schemas('"' ~ db_name ~ '"') %}

{% for schema in schemas[:10] %}
    select '{{ schema }}' as schema_name
    {% if not loop.last %}
    union all
    {% endif %}
{% endfor %}
