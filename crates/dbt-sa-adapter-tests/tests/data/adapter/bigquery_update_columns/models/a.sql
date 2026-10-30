{% set table_name = 'a' %}
{% set db_schema = t_schema_name() %}
{% set db_name = target.database %}

{% do set_up(db_schema) %}

{% set columns  = model.columns %}
{{ log('columns', info=True) }}
{{ print(columns) }}


{% set relation = adapter.get_relation(
    database=db_name,
    schema=db_schema,
    identifier=table_name
) %}

{% do adapter.update_columns(relation, columns) %}

{% set sql %}
SELECT *
FROM {{ db_name }}.{{ db_schema }}.INFORMATION_SCHEMA.COLUMN_FIELD_PATHS
WHERE table_name = "a"
{% endset %}

{% set relation_fields = adapter.execute(sql) %}

select 
    STRUCT('some_value' as b) as a,
    'another_value' as b