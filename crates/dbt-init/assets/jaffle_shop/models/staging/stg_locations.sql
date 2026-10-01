with

source as (

    -- {# This references seed (CSV) data - try switching to {{ source('ecom', 'raw_stores') }} #}
    select * from {{ ref('raw_stores') }}

),

renamed as (

    select

        ----------  ids
        id as location_id,

        ---------- text
        name as location_name,

        ---------- numerics
        tax_rate,

        ---------- timestamps
        cast(opened_at as date) as opened_date

    from source

)

select * from renamed
