SELECT
  SUM(1) AS order_count
FROM "main"."orders" AS o
WHERE o.status = 'completed'
