
{{
    config(
        materialized='incremental',
        databricks_tags={
            var('key_value', 'mode'): var('tag_value', 'initial')
        },
    )
}}


WITH source_data AS (SELECT * FROM {{ ref('a') }} )

{% if is_incremental()  %}

SELECT * FROM source_data WHERE id NOT IN (SELECT id from {{ this }} )

{% else %}

SELECT * FROM source_data where id <= 1

{% endif %}
