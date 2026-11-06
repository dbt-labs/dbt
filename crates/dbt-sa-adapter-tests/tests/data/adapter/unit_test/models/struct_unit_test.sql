SELECT company_id, STRUCT(
    ARRAY_AGG(STRUCT(closing_date, financials.revenue AS amount)) AS revenue,
    ARRAY_AGG(STRUCT(closing_date, financials.debt AS amount)) AS debt
) AS financial_history
FROM {{ ref('stg_struct_unit_test') }}
GROUP BY company_id