{{ config(materialized='view') }}

SELECT 
    campaign_id,
    COUNT(event_id) AS total_num_installs
FROM {{ ref('stg_app_installs') }}
GROUP BY 1
