SELECT
  1337 AS company_id,
  STRUCT(
    [STRUCT(CAST('2025-01-01' AS DATE) AS closing_date, 40 AS amount), STRUCT(CAST('2024-01-01' AS DATE) AS closing_date, 75 AS amount)] AS revenue,
    [STRUCT(CAST('2025-01-01' AS DATE) AS closing_date, 41 AS amount), STRUCT(CAST('2024-01-01' AS DATE) AS closing_date, 76 AS amount)] AS debt
  ) AS financial_history
