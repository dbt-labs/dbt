{{ config(materialized='table') }}

WITH campaign_installs AS (
    -- Get base install metrics per campaign
    SELECT 
        campaign_id,
        total_num_installs,
        CURRENT_TIMESTAMP AS analysis_timestamp
    FROM {{ ref('stg_installs_per_campaign') }}
),

install_rankings AS (
    -- Rank campaigns by install volume
    SELECT 
        campaign_id,
        total_num_installs,
        RANK() OVER (ORDER BY total_num_installs DESC) AS install_rank,
        PERCENT_RANK() OVER (ORDER BY total_num_installs) AS install_percentile
    FROM campaign_installs
),

campaign_summary as (
    select 
        count(distinct campaign_id) as total_campaigns,
        sum(total_num_installs) as total_installs
    from campaign_installs
),

performance_segments AS (
    -- Segment campaigns into performance tiers
    SELECT 
        campaign_id,
        total_num_installs,
        install_rank,
        install_percentile,
        CASE 
            WHEN install_percentile >= 0.75 THEN 'Top Performer'
            WHEN install_percentile >= 0.50 THEN 'Above Average'
            WHEN install_percentile >= 0.25 THEN 'Below Average'
            ELSE 'Low Performer'
        END AS performance_tier
    FROM install_rankings
),

aggregate_metrics AS (
    -- Calculate aggregate statistics
    SELECT 
        performance_tier,
        COUNT(DISTINCT campaign_id) AS campaigns_in_tier,
        SUM(total_num_installs) AS tier_total_installs,
        AVG(total_num_installs) AS tier_avg_installs,
        MIN(total_num_installs) AS tier_min_installs,
        MAX(total_num_installs) AS tier_max_installs
    FROM performance_segments
    GROUP BY performance_tier
),

final_enriched AS (
    -- Join campaign-level and aggregate metrics
    SELECT 
        ps.campaign_id,
        ps.total_num_installs,
        ps.install_rank,
        ps.install_percentile,
        ps.performance_tier,
        am.campaigns_in_tier,
        am.tier_total_installs,
        am.tier_avg_installs,
        am.tier_min_installs,
        am.tier_max_installs,
        ROUND(
            (ps.total_num_installs::NUMERIC / NULLIF(am.tier_avg_installs, 0)) * 100, 
            2
        ) AS pct_of_tier_avg,
        ROUND(
            (ps.total_num_installs::NUMERIC / NULLIF(
                SUM(ps.total_num_installs) OVER (), 0
            )) * 100, 
            2
        ) AS pct_of_total_installs
    FROM performance_segments ps
    LEFT JOIN aggregate_metrics am 
        ON ps.performance_tier = am.performance_tier
)

SELECT * FROM final_enriched
ORDER BY install_rank