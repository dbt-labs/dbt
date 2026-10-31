
{{
    config(
        materialized='incremental',
        unique_key='id',
        tmp_relation_type='transient'
    )
}}

WITH source_data AS (SELECT * FROM {{ ref('model_a') }} )

{% if is_incremental()  %}

SELECT id,
       field1,
       field2,
       field3,
       field4
FROM source_data WHERE id NOT IN (SELECT id from {{ this }} )

{% else %}

SELECT id,
       field1,
       field2
FROM source_data where id <= 1

{% endif %}
