-- Evolved: the columns after order_id swap places, which CREATE OR ALTER can't do.
select o.order_id, {% if var('evolved') %}o.note, o.amount{% else %}o.amount, o.note{% endif %}
from {{ changes(source('raw', 'orders'), 'default') }} as o
qualify row_number() over (
    partition by o.order_id
    order by case when o.METADATA$ACTION = 'INSERT' then 0 else 1 end
) = 1
