{{ config(
    materialized='incremental',
    incremental_strategy='delete+insert',
    unique_key='id',
    dist='id',
    sort=['id']
) }}

select 1 as id, 'a' as val
{% if is_incremental() %}
union all select 2 as id, 'b' as val
{% endif %}
