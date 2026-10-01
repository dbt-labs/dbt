{{ config(materialized='table') }}

WITH platform_installs AS (
    SELECT 
        platform,
        COUNT(DISTINCT customer_id) AS total_installs,
        COUNT(DISTINCT campaign_id) AS campaigns_used,
        MIN(install_time) AS first_install,
        MAX(install_time) AS last_install
    FROM {{ ref('stg_app_installs') }}
    GROUP BY platform
),

platform_engagement AS (
    SELECT 
        platform,
        COUNT(DISTINCT customer_id) AS engaged_users,
        COUNT(DISTINCT event_id) AS total_events,
        COUNT(DISTINCT DATE(event_time)) AS active_days,
        AVG(event_value) AS avg_event_value
    FROM {{ ref('stg_inapp_events') }}
    GROUP BY platform
),

platform_revenue AS (
    SELECT 
        platform,
        COUNT(DISTINCT customer_id) AS paying_customers,
        COUNT(DISTINCT CASE WHEN event_name = 'purchase' THEN event_id END) AS total_purchases,
        SUM(CASE WHEN event_name = 'purchase' THEN event_value ELSE 0 END) AS total_revenue
    FROM {{ ref('stg_inapp_events') }}
    GROUP BY platform
),

final AS (
    SELECT 
        pi.platform,
        pi.total_installs,
        pi.campaigns_used,
        pe.engaged_users,
        pe.total_events,
        pe.active_days,
        pr.paying_customers,
        pr.total_purchases,
        pr.total_revenue,
        {{ calculate_conversion_rate('pr.paying_customers', 'pi.total_installs') }} AS install_to_paying_rate,
        pe.total_events / NULLIF(pi.total_installs, 0) AS avg_events_per_install,
        pr.total_revenue / NULLIF(pr.paying_customers, 0) AS avg_revenue_per_paying_customer,
        DATEDIFF(day, pi.first_install, pi.last_install) AS platform_active_days
    FROM platform_installs pi
    LEFT JOIN platform_engagement pe ON pi.platform = pe.platform
    LEFT JOIN platform_revenue pr ON pi.platform = pr.platform
)

SELECT * FROM final
