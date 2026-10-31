{% set start_date = convert_datetime('20240504', '%Y%m%d') %}

with base AS (
    SELECT 1 as test 
)

SELECT {{ start_date.strftime('%Y') }} as a_year
FROM base