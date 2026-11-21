-- Evolved: note widens from varchar(10) to varchar(50), in place.
select o.order_id, o.amount, cast(o.note as varchar({{ 50 if var('evolved') else 10 }})) as note
from {{ changes(source('raw', 'orders'), 'default') }} as o
qualify row_number() over (
    partition by o.order_id
    order by case when o.METADATA$ACTION = 'INSERT' then 0 else 1 end
) = 1
