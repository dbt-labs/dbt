{{ config(tags=['after_refresh']) }}
-- After apply_changes and a CREATE OR ALTER sync + refresh: order 1 updated, order 2 deleted,
-- order 4 inserted.
with expected as (
    select * from values (1, 15.00, false), (3, 30.00, false), (4, 40.00, false)
        as t (order_id, amount, is_deleted)
),

actual as (
    select order_id, amount, is_deleted from {{ ref('orders_merge') }}
)

(select 'missing' as problem, * from expected minus select 'missing', * from actual)
union all
(select 'unexpected' as problem, * from actual minus select 'unexpected', * from expected)
