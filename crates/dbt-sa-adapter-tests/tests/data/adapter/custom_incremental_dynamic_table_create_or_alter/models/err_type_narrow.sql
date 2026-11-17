-- Evolved: note narrows from varchar(100) to varchar(5).
select o.order_id, o.amount, cast(o.note as varchar({{ 5 if var('evolved') else 100 }})) as note
from {{ changes(source('raw', 'orders'), 'default') }} as o
qualify row_number() over (
    partition by o.order_id
    order by case when o.METADATA$ACTION = 'INSERT' then 0 else 1 end
) = 1
