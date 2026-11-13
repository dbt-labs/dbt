-- this is a arbitrarily chosen warehouse found in the AD_HOC user account
{{ config(
    materialized='table'
) }}

select '{{ target.warehouse }}' as a
