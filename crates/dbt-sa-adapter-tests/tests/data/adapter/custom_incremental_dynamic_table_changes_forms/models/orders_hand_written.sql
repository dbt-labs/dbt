-- CHANGES written by hand after source(). Accepted; under --empty dbt keeps the table name in front of CHANGES.
select o.order_id, o.amount
from {{ source('raw', 'orders') }} changes(information => default) as o
where o.METADATA$ACTION = 'INSERT'
