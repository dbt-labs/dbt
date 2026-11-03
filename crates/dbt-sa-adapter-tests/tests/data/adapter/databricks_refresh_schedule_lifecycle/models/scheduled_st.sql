{{ config(
    materialized='streaming_table',
    schedule=var('st_schedule'),
    on_configuration_change=var('change_policy'),
    tblproperties={'refresh_test_marker': var('st_marker')}
) }}
select id from STREAM({{ source('refresh_input', 'refresh_source') }})
