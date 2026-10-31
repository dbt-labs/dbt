{{
  config(
    materialized="table",
    table_format="iceberg",
    external_volume="s3_iceberg_snow",
    base_location_root="fs_test_user",
    base_location_subpath="ci",
  )
}}

select 1 as world_no, 5 as stage_no, 'iggy' as boss
