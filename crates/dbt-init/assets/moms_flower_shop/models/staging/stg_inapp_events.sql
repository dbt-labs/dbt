{{ config(materialized='view') }}

SELECT 
    event_id,
    customer_id,
    TO_TIMESTAMP(event_time*1000) AS event_time,  
    event_name,
    event_value,
    additional_details,
    platform,
    campaign_id
FROM {{ ref('raw_inapp_events') }}