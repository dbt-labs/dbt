SELECT
    id,
    event_timestamp,
    local_datetime,
    UNIX_MICROS(event_timestamp) AS utc_us,
    EXTRACT(HOUR FROM local_datetime) AS wall_hour
FROM {{ ref('events') }}
