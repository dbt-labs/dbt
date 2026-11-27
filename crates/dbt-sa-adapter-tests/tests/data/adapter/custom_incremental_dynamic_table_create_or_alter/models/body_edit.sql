-- Evolved: appends amount_cents. It applies to changes made after the edit; rows already merged keep NULL.
select o.order_id, o.amount{% if var('evolved') %}, o.amount * 100 as amount_cents{% endif %}
from {{ changes(source('raw', 'orders'), 'default') }} as o
qualify row_number() over (
    partition by o.order_id
    order by case when o.METADATA$ACTION = 'INSERT' then 0 else 1 end
) = 1
