{{ config(tags=['after_switch_back']) }}
-- After CI DT -> regular DT -> CI DT: rebuilt from the source's current rows.
with expected as (
    select * from values (1, 15.00, false), (3, 30.00, false), (4, 40.00, false), (5, 50.00, false)
        as t (order_id, amount, is_deleted)
),

actual as (
    select order_id, amount, is_deleted from {{ ref('orders_merge') }}
)

(select 'missing' as problem, * from expected minus select 'missing', * from actual)
union all
(select 'unexpected' as problem, * from actual minus select 'unexpected', * from expected)
