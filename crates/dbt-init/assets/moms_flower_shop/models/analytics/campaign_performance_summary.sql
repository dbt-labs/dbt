{{ config(materialized='table') }}

WITH campaign_installs AS (
    SELECT 
        campaign_id,
        campaign_name,
        campaign_type,
        COUNT(DISTINCT customer_id) AS total_installs,
        COUNT(DISTINCT event_id) AS total_install_events,
        MIN(install_time) AS first_install_date,
        MAX(install_time) AS last_install_date
    FROM {{ ref('stg_app_installs') }}
    WHERE campaign_id != -1
    GROUP BY campaign_id, campaign_name, campaign_type
),

campaign_costs AS (
    SELECT 
        campaign_id,
        SUM(cost) AS total_spend,
        COUNT(DISTINCT event_id) AS total_ad_events,
        AVG(cost) AS avg_cost_per_event
    FROM {{ ref('raw_marketing_campaign_events') }}
    GROUP BY campaign_id
),

post_install_purchases AS (
    SELECT 
        i.campaign_id,
        COUNT(DISTINCT e.customer_id) AS purchasers,
        SUM(e.event_value) AS total_revenue,
        COUNT(DISTINCT e.event_id) AS total_purchases
    FROM {{ ref('stg_app_installs') }} i
    INNER JOIN {{ ref('stg_inapp_events') }} e 
        ON i.customer_id = e.customer_id
        AND e.event_time > i.install_time
        AND e.event_name = 'purchase'
    WHERE i.campaign_id != -1
    GROUP BY i.campaign_id
),

final AS (
    SELECT 
        ci.campaign_id,
        ci.campaign_name,
        ci.campaign_type,
        ci.total_installs,
        cc.total_spend,
        cc.avg_cost_per_event,
        pip.purchasers,
        pip.total_revenue,
        pip.total_purchases,
        {{ calculate_conversion_rate('pip.purchasers', 'ci.total_installs') }} AS install_to_purchase_rate,
        cc.total_spend / NULLIF(ci.total_installs, 0) AS cost_per_install,
        pip.total_revenue / NULLIF(cc.total_spend, 0) AS return_on_ad_spend,
        DATEDIFF(day, ci.first_install_date, ci.last_install_date) AS campaign_duration_days
    FROM campaign_installs ci
    LEFT JOIN campaign_costs cc ON ci.campaign_id = cc.campaign_id
    LEFT JOIN post_install_purchases pip ON ci.campaign_id = pip.campaign_id
)

SELECT * FROM final
