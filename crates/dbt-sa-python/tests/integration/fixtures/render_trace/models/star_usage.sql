with base_values as (
    select * from {{ ref('base') }}
)

select
    {{ dbt_utils.star(
        from=ref('base'),
        relation_alias='base_values',
        except=['internal_note'],
        prefix='copied_'
    ) }}
from base_values
