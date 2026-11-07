-- After apply_changes and a refresh. APPEND_ONLY changes carry inserts only, so the update of
-- order 1 and the delete of order 2 are not seen: every order ever inserted, at its inserted amount.
with expected as (
    select * from values (1, 10.00), (2, 20.00), (3, 30.00), (4, 40.00)
        as t (order_id, amount)
),

actual as (
    select order_id, amount from {{ ref('orders_transient') }}
)

(select 'missing' as problem, * from expected minus select 'missing', * from actual)
union all
(select 'unexpected' as problem, * from actual minus select 'unexpected', * from expected)
