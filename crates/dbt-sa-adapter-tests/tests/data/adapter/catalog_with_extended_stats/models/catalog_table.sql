-- Materializes as a TABLE so it has a row in svv_table_info, proving the
-- extended-catalog join (#15451) attaches real stats (has_stats: true)
-- instead of just gracefully degrading like a view.
{{ config(materialized='table') }}
select 1 as id, 'alice' as name
