-- After apply_changes and a refresh: (1, A) updated, (1, B) deleted, (2, C) inserted.
with expected as (
    select * from values (1, 'A', 80, false), (2, 'A', 10, false), (2, 'C', 5, false)
        as t (warehouse_id, sku, quantity, is_deleted)
),

actual as (
    select warehouse_id, sku, quantity, is_deleted from {{ ref('stock_merge') }}
)

(select 'missing' as problem, * from expected minus select 'missing', * from actual)
union all
(select 'unexpected' as problem, * from actual minus select 'unexpected', * from expected)
