{{ config(materialized='table') }}

SELECT 
    -- install events data
    DATE(install_time) AS install_date,
    campaign_name,
    platform,
    COUNT(DISTINCT customer_id) AS distinct_installs
FROM {{ ref('stg_app_installs') }}
GROUP BY 1,2,3