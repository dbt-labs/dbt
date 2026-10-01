SELECT
  o.customer_id AS customer,
  SUM(1) AS order_count
FROM "main"."orders" AS o
GROUP BY 1
