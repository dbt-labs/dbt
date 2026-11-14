{{ config(tags=['after_body_edit']) }}
-- After the body edit (appends amount_cents) and one more insert. CREATE OR ALTER keeps the
-- existing rows (no rebuild), so only order 5, changed after the edit, has amount_cents.
with expected as (
    select * from values
        (1, 15.00, null::number), (3, 30.00, null::number), (4, 40.00, null::number), (5, 50.00, 5000)
        as t (order_id, amount, amount_cents)
),

actual as (
    select order_id, amount, amount_cents from {{ ref('orders_merge') }}
)

(select 'missing' as problem, * from expected minus select 'missing', * from actual)
union all
(select 'unexpected' as problem, * from actual minus select 'unexpected', * from expected)
