{{ config(tags=['after_empty']) }}
-- After dbt run --empty (and the manual refresh scheduler = 'DISABLE' implies), every form is
-- built empty although the sources have rows.
select 'orders_macro' as model, order_id from {{ ref('orders_macro') }}
union all
select 'orders_hand_written', order_id from {{ ref('orders_hand_written') }}
union all
select 'orders_with_region', order_id from {{ ref('orders_with_region') }}
