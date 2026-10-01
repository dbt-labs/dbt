SELECT
  DATE_TRUNC('day', o.ordered_at)::DATE AS metric_time,
  SUM(o.amount) AS revenue
FROM "main"."orders" AS o
WHERE CAST(o.ordered_at AS TIMESTAMP) >= CAST('2024-01-01' AS TIMESTAMP)
  AND CAST(o.ordered_at AS TIMESTAMP) < CAST('2024-01-02' AS TIMESTAMP) + INTERVAL '1 day'
GROUP BY 1
