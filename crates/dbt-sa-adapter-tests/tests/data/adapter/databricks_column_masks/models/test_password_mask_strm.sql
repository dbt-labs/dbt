{{ config(
    materialized='streaming_table',
) }}
SELECT * FROM stream {{ ref('base_model_seed') }}