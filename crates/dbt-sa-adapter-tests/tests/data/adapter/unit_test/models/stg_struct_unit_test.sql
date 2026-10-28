SELECT
    1 AS company_id,
    CAST('2025-01-01' AS DATE) AS closing_date,
    STRUCT(
        100 AS revenue,
        99 AS debt
    ) AS financials
UNION ALL
SELECT
    1,
    CAST('2024-01-01' AS DATE) AS closing_date,
    STRUCT(
        75 AS revenue,
        76 AS debt
    ) AS financials