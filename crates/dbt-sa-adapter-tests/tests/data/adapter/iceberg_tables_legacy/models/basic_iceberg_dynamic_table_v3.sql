{{
  config(
    materialized="dynamic_table",
    snowflake_warehouse='transforming',
    target_lag='2 hour',
    table_format="iceberg",
    external_volume="s3_iceberg_snow",
    base_location_root="fs_test_user",
    base_location_subpath="ci",
    iceberg_version=3,
  )
}}

select world_no from {{ ref('basic_iceberg_table') }}
