-- After --full-refresh. Creating a CI DT reads the source's current rows as inserts (it does not
-- replay the change history), so the APPEND_ONLY model now holds the source as it is: order 1
-- and order 4 updated, order 2 re-inserted.
with expected as (
    select * from values (1, 15.00), (2, 25.00), (3, 30.00), (4, 45.00)
        as t (order_id, amount)
),

actual as (
    select order_id, amount from {{ ref('orders_transient') }}
)

(select 'missing' as problem, * from expected minus select 'missing', * from actual)
union all
(select 'unexpected' as problem, * from actual minus select 'unexpected', * from expected)
