{{ config(
    materialized='view',
    tags=['dashboard', 'executive']
) }}

WITH campaign_summary AS (
    SELECT 
        c.campaign_id,
        c.campaign_name,
        c.campaign_type,
        c.total_installs,
        c.total_spend,
        c.return_on_ad_spend
    FROM {{ ref('campaign_performance_summary') }} c
),

campaign_retention AS (
    SELECT 
        campaign_id,
        day_7_retention_rate,
        day_30_retention_rate
    FROM {{ ref('campaign_comparison') }}
),

campaign_ltv AS (
    SELECT 
        campaign_id,
        avg_ltv,
        ltv_to_cac_ratio,
        customer_monetization_rate
    FROM {{ ref('customer_acquisition_cost') }}
),

final AS (
    SELECT 
        cs.campaign_id,
        cs.campaign_name,
        cs.campaign_type,
        cs.total_installs,
        cs.total_spend,
        cs.return_on_ad_spend,
        cr.day_7_retention_rate,
        cr.day_30_retention_rate,
        cl.avg_ltv,
        cl.ltv_to_cac_ratio,
        cl.customer_monetization_rate,
        CASE 
            WHEN cl.ltv_to_cac_ratio >= 3 AND cr.day_30_retention_rate >= 40 THEN 'Excellent'
            WHEN cl.ltv_to_cac_ratio >= 2 AND cr.day_30_retention_rate >= 30 THEN 'Good'
            WHEN cl.ltv_to_cac_ratio >= 1 AND cr.day_30_retention_rate >= 20 THEN 'Fair'
            ELSE 'Poor'
        END AS campaign_grade
    FROM campaign_summary cs
    LEFT JOIN campaign_retention cr ON cs.campaign_id = cr.campaign_id
    LEFT JOIN campaign_ltv cl ON cs.campaign_id = cl.campaign_id
)

SELECT * FROM final
