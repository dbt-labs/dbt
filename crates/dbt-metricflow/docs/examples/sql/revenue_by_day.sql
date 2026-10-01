SELECT
  DATE_TRUNC('day', o.ordered_at)::DATE AS metric_time,
  SUM(o.amount) AS revenue
FROM "main"."orders" AS o
GROUP BY 1
