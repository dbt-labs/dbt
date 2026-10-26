{% set table_name = 'a' %}
{% set db_schema = t_schema_name() %}
{% set db_name = target.database %}

{% do set_up(db_schema, table_name) %}

{% set relation = ref('a') %}

{% set query %}
    select * from {{ ref('a') }}
{% endset %}

{{ print("before truncate_relation") }}
{% set table = run_query(query) %}
{{ print(table) }}

{% set result = adapter.truncate_relation(relation) %}

{{ print("after truncate_relation") }}
{% set table = run_query(query) %}
{{ print(table) }}

SELECT * FROM {{ ref('a') }}
