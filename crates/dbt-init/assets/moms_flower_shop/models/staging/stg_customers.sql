{{ config(materialized='view') }}

SELECT 
    c.id AS customer_id,
    c.first_name,
    c.last_name,
    c.first_name || ' ' || c.last_name AS full_name,
    c.email,
    c.gender,
    
    -- Marketing info
    i.campaign_id,
    i.campaign_name,
    i.campaign_type,
    i.platform,

    -- Address info
    c.address_id,
    a.full_address,
    a.city,
    a.state
FROM {{ ref('raw_customers') }} c 

    LEFT OUTER JOIN {{ ref('stg_app_installs') }} i
        ON (c.id = i.customer_id)

    LEFT OUTER JOIN {{ ref('raw_addresses') }} a
        ON (c.address_id = a.address_id)
