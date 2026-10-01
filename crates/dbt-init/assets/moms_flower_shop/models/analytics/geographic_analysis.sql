{{ config(materialized='table') }}

WITH customer_locations AS (
    SELECT 
        c.customer_id,
        c.state,
        c.city,
        a.full_address
    FROM {{ ref('stg_customers') }} c
    LEFT JOIN {{ ref('raw_addresses') }} a ON c.address_id = a.address_id
),

state_installs AS (
    SELECT 
        cl.state,
        COUNT(DISTINCT i.customer_id) AS total_installs,
        COUNT(DISTINCT i.campaign_id) AS unique_campaigns
    FROM {{ ref('stg_app_installs') }} i
    INNER JOIN customer_locations cl ON i.customer_id = cl.customer_id
    WHERE cl.state IS NOT NULL
    GROUP BY cl.state
),

state_revenue AS (
    SELECT 
        cl.state,
        COUNT(DISTINCT e.customer_id) AS paying_customers,
        SUM(CASE WHEN e.event_name = 'purchase' THEN e.event_value ELSE 0 END) AS total_revenue,
        COUNT(DISTINCT CASE WHEN e.event_name = 'purchase' THEN e.event_id END) AS total_purchases
    FROM {{ ref('stg_inapp_events') }} e
    INNER JOIN customer_locations cl ON e.customer_id = cl.customer_id
    WHERE cl.state IS NOT NULL
    GROUP BY cl.state
),

final AS (
    SELECT 
        si.state,
        si.total_installs,
        si.unique_campaigns,
        sr.paying_customers,
        sr.total_purchases,
        sr.total_revenue,
        {{ calculate_conversion_rate('sr.paying_customers', 'si.total_installs') }} AS conversion_rate,
        sr.total_revenue / NULLIF(si.total_installs, 0) AS revenue_per_install,
        sr.total_revenue / NULLIF(sr.paying_customers, 0) AS avg_customer_value
    FROM state_installs si
    LEFT JOIN state_revenue sr ON si.state = sr.state
)

SELECT * FROM final
ORDER BY total_revenue DESC
