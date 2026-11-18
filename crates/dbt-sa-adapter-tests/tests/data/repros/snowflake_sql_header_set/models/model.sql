{{
    config(
        materialized='table'
    )
}}

{% call set_sql_header(config) %}
    SET ts = '1900-01-01'::TIMESTAMP;
{% endcall %}

WITH events AS (
    SELECT 1 as id, CURRENT_TIMESTAMP() as created_at
)
SELECT id, created_at, $ts as declared_ts
FROM events
WHERE created_at >= $ts
