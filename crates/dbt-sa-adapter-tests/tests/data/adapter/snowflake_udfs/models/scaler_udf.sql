SELECT IS_EVEN(ID) AS IS_EVEN FROM {{ ref('base') }}
UNION ALL
SELECT {{target.schema }}.IS_EVEN(ID) AS IS_EVEN FROM {{ ref('base') }}
UNION ALL
SELECT {{ target.database }}.{{target.schema }}.IS_EVEN(ID) AS IS_EVEN FROM {{ ref('base') }}
