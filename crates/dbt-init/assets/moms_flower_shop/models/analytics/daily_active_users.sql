{{ config(materialized='table') }}

WITH daily_events AS (
    SELECT 
        DATE(event_time) AS event_date,
        customer_id,
        platform,
        COUNT(DISTINCT event_id) AS events_per_user
    FROM {{ ref('stg_inapp_events') }}
    GROUP BY DATE(event_time), customer_id, platform
),

daily_aggregates AS (
    SELECT 
        event_date,
        platform,
        COUNT(DISTINCT customer_id) AS daily_active_users,
        SUM(events_per_user) AS total_events,
        AVG(events_per_user) AS avg_events_per_user
    FROM daily_events
    GROUP BY event_date, platform
),

weekly_users AS (
    SELECT 
        DATE_TRUNC('week', event_time) AS week_start,
        platform,
        COUNT(DISTINCT customer_id) AS weekly_active_users
    FROM {{ ref('stg_inapp_events') }}
    GROUP BY DATE_TRUNC('week', event_time), platform
),

monthly_users AS (
    SELECT 
        DATE_TRUNC('month', event_time) AS month_start,
        platform,
        COUNT(DISTINCT customer_id) AS monthly_active_users
    FROM {{ ref('stg_inapp_events') }}
    GROUP BY DATE_TRUNC('month', event_time), platform
),

final AS (
    SELECT 
        da.event_date,
        da.platform,
        da.daily_active_users,
        da.total_events,
        da.avg_events_per_user,
        wu.weekly_active_users,
        mu.monthly_active_users
    FROM daily_aggregates da
    LEFT JOIN weekly_users wu 
        ON DATE_TRUNC('week', da.event_date) = wu.week_start
        AND da.platform = wu.platform
    LEFT JOIN monthly_users mu 
        ON DATE_TRUNC('month', da.event_date) = mu.month_start
        AND da.platform = mu.platform
)

SELECT * FROM final
