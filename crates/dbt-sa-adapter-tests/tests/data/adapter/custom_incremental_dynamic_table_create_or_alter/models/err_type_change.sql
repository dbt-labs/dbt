-- Evolved: amount changes from a number to a varchar.
select o.order_id, {% if var('evolved') %}cast(o.amount as varchar(20)){% else %}o.amount{% endif %} as amount, o.note
from {{ changes(source('raw', 'orders'), 'default') }} as o
qualify row_number() over (
    partition by o.order_id
    order by case when o.METADATA$ACTION = 'INSERT' then 0 else 1 end
) = 1
