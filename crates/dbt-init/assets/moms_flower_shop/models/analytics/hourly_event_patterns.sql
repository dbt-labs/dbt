{{ config(materialized='table') }}

WITH hourly_breakdown AS (
    SELECT 
        DATE(event_time) AS event_date,
        HOUR(event_time) AS event_hour,
        DAYOFWEEK(event_time) AS day_of_week,
        platform,
        event_name,
        COUNT(DISTINCT customer_id) AS unique_users,
        COUNT(event_id) AS event_count,
        AVG(event_value) AS avg_event_value
    FROM {{ ref('stg_inapp_events') }}
    GROUP BY 
        DATE(event_time),
        HOUR(event_time),
        DAYOFWEEK(event_time),
        platform,
        event_name
),

hourly_aggregates AS (
    SELECT 
        event_hour,
        day_of_week,
        platform,
        event_name,
        AVG(unique_users) AS avg_unique_users,
        AVG(event_count) AS avg_event_count,
        SUM(event_count) AS total_events
    FROM hourly_breakdown
    GROUP BY event_hour, day_of_week, platform, event_name
),

peak_hours AS (
    SELECT 
        platform,
        event_name,
        MAX(avg_event_count) AS peak_event_count
    FROM hourly_aggregates
    GROUP BY platform, event_name
),

final AS (
    SELECT 
        ha.event_hour,
        ha.day_of_week,
        CASE ha.day_of_week
            WHEN 0 THEN 'Sunday'
            WHEN 1 THEN 'Monday'
            WHEN 2 THEN 'Tuesday'
            WHEN 3 THEN 'Wednesday'
            WHEN 4 THEN 'Thursday'
            WHEN 5 THEN 'Friday'
            WHEN 6 THEN 'Saturday'
        END AS day_name,
        ha.platform,
        ha.event_name,
        ha.avg_unique_users,
        ha.avg_event_count,
        ha.total_events,
        CASE WHEN ha.avg_event_count = ph.peak_event_count THEN TRUE ELSE FALSE END AS is_peak_hour
    FROM hourly_aggregates ha
    LEFT JOIN peak_hours ph 
        ON ha.platform = ph.platform 
        AND ha.event_name = ph.event_name
)

SELECT * FROM final
ORDER BY platform, event_name, day_of_week, event_hour
