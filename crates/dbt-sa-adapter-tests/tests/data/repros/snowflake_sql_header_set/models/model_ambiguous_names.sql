{{
    config(
        materialized='table'
    )
}}

{% call set_sql_header(config) %}
    SET (date, timestamp, value, type, offset, count) = (
        CURRENT_DATE(),
        CURRENT_TIMESTAMP(),
        'hello',
        'event',
        10,
        42
    );
{% endcall %}

SELECT
    $date as declared_date,
    $timestamp as declared_timestamp,
    $value as declared_value,
    $type as declared_type,
    $offset as declared_offset,
    $count as declared_count
