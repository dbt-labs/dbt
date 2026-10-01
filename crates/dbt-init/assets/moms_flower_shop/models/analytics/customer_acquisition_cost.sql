{{ config(materialized='table') }}

WITH campaign_spend AS (
    SELECT 
        campaign_id,
        campaign_name,
        c_name AS campaign_type,
        SUM(cost) AS total_campaign_cost,
        COUNT(DISTINCT event_id) AS marketing_events,
        MIN(TO_TIMESTAMP(event_time/1000)) AS campaign_start_date,
        MAX(TO_TIMESTAMP(event_time/1000)) AS campaign_end_date
    FROM {{ ref('raw_marketing_campaign_events') }}
    GROUP BY campaign_id, campaign_name, c_name
),

campaign_acquisitions AS (
    SELECT 
        campaign_id,
        COUNT(DISTINCT customer_id) AS acquired_customers,
        MIN(install_time) AS first_acquisition,
        MAX(install_time) AS last_acquisition
    FROM {{ ref('stg_app_installs') }}
    WHERE campaign_id != -1
    GROUP BY campaign_id
),

customer_ltv AS (
    SELECT 
        i.campaign_id,
        SUM(CASE WHEN e.event_name = 'purchase' THEN e.event_value ELSE 0 END) AS total_customer_revenue,
        COUNT(DISTINCT e.customer_id) AS revenue_generating_customers
    FROM {{ ref('stg_app_installs') }} i
    INNER JOIN {{ ref('stg_inapp_events') }} e ON i.customer_id = e.customer_id
    WHERE i.campaign_id != -1
    GROUP BY i.campaign_id
),

final AS (
    SELECT 
        cs.campaign_id,
        cs.campaign_name,
        cs.campaign_type,
        cs.total_campaign_cost,
        cs.marketing_events,
        ca.acquired_customers,
        cl.total_customer_revenue,
        cl.revenue_generating_customers,
        cs.total_campaign_cost / NULLIF(ca.acquired_customers, 0) AS cac,
        cl.total_customer_revenue / NULLIF(cl.revenue_generating_customers, 0) AS avg_ltv,
        (cl.total_customer_revenue / NULLIF(cl.revenue_generating_customers, 0)) / 
            NULLIF(cs.total_campaign_cost / NULLIF(ca.acquired_customers, 0), 0) AS ltv_to_cac_ratio,
        DATEDIFF(day, cs.campaign_start_date, cs.campaign_end_date) AS campaign_duration_days,
        {{ calculate_conversion_rate('cl.revenue_generating_customers', 'ca.acquired_customers') }} AS customer_monetization_rate
    FROM campaign_spend cs
    LEFT JOIN campaign_acquisitions ca ON cs.campaign_id = ca.campaign_id
    LEFT JOIN customer_ltv cl ON cs.campaign_id = cl.campaign_id
)

SELECT * FROM final
ORDER BY ltv_to_cac_ratio DESC
