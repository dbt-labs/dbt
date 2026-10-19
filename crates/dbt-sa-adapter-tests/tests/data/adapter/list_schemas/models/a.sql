{% set table_name = 'a' %}
{% set db_schema = t_schema_name() %}
{% set db_name = target.database %}

{% do set_up(db_schema, table_name) %}

{{ print(db_name) }}
{{ print(db_schema) }}

{% set schemas = adapter.list_schemas(db_name) %}

{{ print('schemas') }}

{# commented out to avoid stdout diff #}
{# {{ print(schemas) }} #}

{% for schema in schemas[:10] %}
    select '{{ schema }}' as schema_name
    {% if not loop.last %}
    union all
    {% endif %}
{% endfor %}
