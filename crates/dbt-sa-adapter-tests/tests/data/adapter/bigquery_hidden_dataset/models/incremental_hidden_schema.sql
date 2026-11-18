{{
    config(
        materialized='incremental',
        unique_key='id',
        incremental_strategy='merge',
        full_refresh=false
    )
}}

with source as (
    select id, current_timestamp() as updated_at
    from unnest([1, 2, 3]) as id
)

select * from source

{% if is_incremental() %}
where updated_at > (select max(updated_at) from {{ this }})
{% endif %}
