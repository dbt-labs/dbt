{{ config(
    materialized='incremental',
    unique_key='id',
    table_format='iceberg',
    external_volume='s3_iceberg_snow',
    base_location_root="fs_test_user",
    base_location_subpath="ci",
) }}

with new_data as (
    select 1 as id, '2025-09-30'::date as updated_at, 'alpha' as some_value
    union all
    select 2 as id, '2025-09-29'::date as updated_at, 'beta' as some_value
    union all
    select 3 as id, '2025-09-28'::date as updated_at, 'gamma' as some_value

)

select *
from new_data

{% if is_incremental() %}
  where updated_at > (
    select coalesce(max(updated_at), '1900-01-01'::date) from {{ this }}
  )
{% endif %}
