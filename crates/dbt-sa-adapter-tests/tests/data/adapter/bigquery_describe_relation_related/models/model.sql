{% set db_name = target.database %}
{% set db_schema = t_schema_name() %}

{% do set_up(db_schema) %}

{% set a = api.Relation.create(db_name, db_schema, "a", "materialized_view") %}
{% set a_result = adapter.describe_relation(a) %}
{{ print("time partitioning") }}
{{ print(a_result) }}

{% set b = api.Relation.create(db_name, db_schema, "b", "materialized_view") %}
{% set b_result = adapter.describe_relation(b) %}
{{ print("range partitioning") }}
{{ print(b_result) }}

{% set is_replaceable = adapter.is_replaceable(a, config.get('partition', none), config.get('cluster', none)) %}
{{ print("is replaceable returns false") }}
{{ print(is_replaceable) }}

{% set is_replaceable = adapter.is_replaceable(a, {
    "field": "user_id",
    "data_type": "int64",
    "range": {
        "start": 0,
        "end": 100,
        "interval": 10,
    }
}, none) %}
{{ print("is replaceable returns true") }}
{{ print(is_replaceable) }}

SELECT 1 as one
