SELECT
  o.status AS status,
  SUM(1) AS order_count
FROM "main"."orders" AS o
GROUP BY 1
