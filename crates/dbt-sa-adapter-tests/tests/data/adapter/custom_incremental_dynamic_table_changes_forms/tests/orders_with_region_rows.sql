-- After --full-refresh, apply_changes and a refresh, joined to the ephemeral customers_dim.
with expected as (
    select * from values (1, 15.00, 'EU'), (2, 20.00, 'US'), (3, 30.00, 'APAC')
        as t (order_id, amount, region)
),

actual as (
    select order_id, amount, region from {{ ref('orders_with_region') }}
)

(select 'missing' as problem, * from expected minus select 'missing', * from actual)
union all
(select 'unexpected' as problem, * from actual minus select 'unexpected', * from expected)
