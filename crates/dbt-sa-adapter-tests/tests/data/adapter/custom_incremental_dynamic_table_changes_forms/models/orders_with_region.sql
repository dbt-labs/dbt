-- Joins an ephemeral model: column discovery runs the compiled SQL, which carries its CTE.
select o.order_id, o.amount, c.region
from {{ changes(source('raw', 'orders'), 'default') }} as o
join {{ ref('customers_dim') }} as c on o.customer_id = c.customer_id
where o.METADATA$ACTION = 'INSERT'
