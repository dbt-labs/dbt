-- Test that snowflake_warehouse config is respected
{{ config(
    materialized='table',
    snowflake_warehouse='DBT_TESTING_ALT'
) }}

select '{{ config.snowflake_warehouse }}' as a
