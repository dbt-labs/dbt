with

source as (

    -- {# This references seed (CSV) data - try switching to {{ source('ecom', 'raw_items') }} #}
    select * from {{ ref('raw_items') }}

),

renamed as (

    select

        ----------  ids
        id as order_item_id,
        order_id,
        sku as product_id

    from source

)

select * from renamed
