{{ config(
    partition_by=var('partition_by', none),
    use_safer_relation_operations=var('safe', false)
) }}

{% set source_table = 'source_table' %}

{% set source_relation = adapter.get_relation(target.database, target.schema, source_table) %}

SELECT * FROM {{ source_relation }}