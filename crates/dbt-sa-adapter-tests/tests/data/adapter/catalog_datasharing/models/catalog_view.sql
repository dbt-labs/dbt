-- Materializes as a VIEW so the SHOW TABLES subtype to catalog table_type
-- mapping (VIEW) is exercised alongside the table.
{{ config(materialized='view') }}
select id, name from {{ ref('catalog_table') }}
