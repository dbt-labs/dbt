{{ config(
    materialized = 'table',
    table_format = 'iceberg',
    location_root = 's3_iceberg_snow',
    catalog_name = 'e2e',
) }}

{%- set tblproperties = adapter.update_tblproperties_for_uniform_iceberg(config,) -%}
{{ print(tblproperties) }}

select 1 as id, 'hello iceberg' as intro
