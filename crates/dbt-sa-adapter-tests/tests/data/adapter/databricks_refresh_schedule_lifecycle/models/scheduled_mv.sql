{{ config(
    materialized='materialized_view',
    schedule=var('mv_schedule'),
    on_configuration_change=var('change_policy')
) }}
select id from {{ source('refresh_input', 'refresh_source') }}
