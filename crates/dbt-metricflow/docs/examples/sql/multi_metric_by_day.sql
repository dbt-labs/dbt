SELECT
  DATE_TRUNC('day', o.ordered_at)::DATE AS metric_time,
  SUM(1) AS order_count,
  SUM(o.amount) AS revenue
FROM "main"."orders" AS o
GROUP BY 1
