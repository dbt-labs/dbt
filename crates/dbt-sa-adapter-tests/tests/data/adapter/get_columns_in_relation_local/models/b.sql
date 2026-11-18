{# this table name is the same as the table name from a.sql #}
{% set table_name = 'a' %}
{% set db_name = target.database %}
{% set db_schema = t_schema_name() %}

{% do set_up(db_schema, table_name) %}

{{ print(db_name) }}
{{ print(db_schema) }}

{% set relation = api.Relation.create(db_name, db_schema, table_name) %}
{% set columns = adapter.get_columns_in_relation(relation) %}

{% for col in columns %}
    {% if col.name | lower == 'number_col' or col.name | lower == 'text_col' %}
        {% do log("Remote Column: " ~ col, info=True) %}
    {% else %}
        {% do log("Local Column: " ~ col, info=True) %}
    {% endif %}
{% endfor %}

-- Comment to ensure that b runs after a {{ ref('a') }}

select 1 as one
