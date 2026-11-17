-- Evolved: note is dropped. CREATE OR ALTER drops the column in place, with its data.
select o.order_id, o.amount{% if not var('evolved') %}, o.note{% endif %}
from {{ changes(source('raw', 'orders'), 'default') }} as o
qualify row_number() over (
    partition by o.order_id
    order by case when o.METADATA$ACTION = 'INSERT' then 0 else 1 end
) = 1
