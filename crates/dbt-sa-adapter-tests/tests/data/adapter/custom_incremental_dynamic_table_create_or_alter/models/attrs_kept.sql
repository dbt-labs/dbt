-- Evolved: cluster_by changes. CREATE OR ALTER changes it in place and keeps the tag, policy and grant set out of band.
{{ config(cluster_by=['amount' if var('evolved') else 'order_id']) }}
select o.order_id, o.amount
from {{ changes(source('raw', 'orders'), 'default') }} as o
qualify row_number() over (
    partition by o.order_id
    order by case when o.METADATA$ACTION = 'INSERT' then 0 else 1 end
) = 1
