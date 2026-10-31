{% set db_schema = t_schema_name() %}
{% set db_name = target.database %}

{% do set_up(db_schema) %}

{{ print(db_name) }}
{{ print(db_schema) }}

{% set columns = adapter.get_missing_columns(ref('a'), ref('b')) %}
{% for col in columns %}
    {% do log("Column: " ~ col.name, info=True) %}
{% endfor %}

SELECT 1 as one
