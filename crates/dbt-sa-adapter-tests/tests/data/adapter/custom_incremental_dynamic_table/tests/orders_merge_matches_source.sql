-- A merge CI DT over DEFAULT changes holds the source's current rows, after each refresh and
-- rebuild: after apply_changes, order 1 updated, order 2 deleted, order 4 inserted; after
-- apply_more_changes, order 2 re-inserted and order 4 updated.
with expected as (
    select order_id, customer_id, amount, false as is_deleted from {{ source('raw', 'orders') }}
),

actual as (
    select order_id, customer_id, amount, is_deleted from {{ ref('orders_merge') }}
)

(select 'missing' as problem, * from expected minus select 'missing', * from actual)
union all
(select 'unexpected' as problem, * from actual minus select 'unexpected', * from expected)
