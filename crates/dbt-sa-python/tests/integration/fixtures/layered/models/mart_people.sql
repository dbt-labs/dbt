select id, upper(name) as name from {{ ref('stg_people') }}
