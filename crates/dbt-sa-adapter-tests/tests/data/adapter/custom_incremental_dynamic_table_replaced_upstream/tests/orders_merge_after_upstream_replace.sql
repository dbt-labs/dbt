-- After the second run replaced stg_orders. The replacement's change history starts empty, so the
-- refresh reads all its rows as inserts: order 1 updated, order 3 inserted. Order 2, deleted
-- upstream, is not in the new table and so is never seen as deleted: it stays.
with expected as (
    select * from values (1, 15.00), (2, 20.00), (3, 30.00)
        as t (order_id, amount)
),

actual as (
    select order_id, amount from {{ ref('orders_merge') }}
)

(select 'missing' as problem, * from expected minus select 'missing', * from actual)
union all
(select 'unexpected' as problem, * from actual minus select 'unexpected', * from expected)
