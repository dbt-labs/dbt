-- After CREATE OR ALTER widened the column and a refresh: the long note arrived, the short one kept.
with expected as (
    select * from values (1, 'short'), (2, 'a much longer note value') as t (order_id, note)
),

actual as (
    select order_id, note from {{ ref('orders_merge') }}
)

(select 'missing' as problem, * from expected minus select 'missing', * from actual)
union all
(select 'unexpected' as problem, * from actual minus select 'unexpected', * from expected)
