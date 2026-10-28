-- Materializes as a VIEW, which never appears in svv_table_info, so it
-- should still show has_stats: false alongside catalog_table's real stats.
{{ config(materialized='view') }}
select id, name from {{ ref('catalog_table') }}
