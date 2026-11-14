-- CHANGES written by hand after source(): discovery runs it as written (not under --empty).
select o.order_id, o.amount
from {{ source('raw', 'orders') }} changes(information => default) as o
where o.METADATA$ACTION = 'INSERT'
