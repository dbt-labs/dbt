-- The recommended spelling: the changes() macro.
select o.order_id, o.amount
from {{ changes(source('raw', 'orders'), 'default') }} as o
where o.METADATA$ACTION = 'INSERT'
