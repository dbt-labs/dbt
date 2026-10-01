{{ config(materialized='view') }}

SELECT 
    DISTINCT
    -- install events data
    i.event_id,
    i.customer_id,
    i.event_time AS install_time,
    i.platform,

    -- marketing campaigns data - if doesn't exist than organic
    COALESCE(m.campaign_id, -1) AS campaign_id, 
    COALESCE(m.campaign_name, 'organic') AS campaign_name,
    COALESCE(m.c_name, 'organic') AS campaign_type
FROM {{ ref('stg_inapp_events') }} i 
    JOIN {{ ref('raw_marketing_campaign_events') }} m
        ON (i.campaign_id = m.campaign_id) 
WHERE event_name = 'install'