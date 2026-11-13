SELECT
    company_id,
    revenue * 2 AS doubled_revenue
FROM {{ ref('stg_input') }}
