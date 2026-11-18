-- Materializes as a TABLE so `run --write-catalog` catalogs it via
-- SHOW TABLES + svv_redshift_columns under datasharing.
{{ config(materialized='table') }}
select 1 as id, 'alice' as name
