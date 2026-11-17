{{
    config(materialized="table")
}}

SELECT
  1 AS id,
  'milkshake' AS product,
  4.99 AS price,
  TIMESTAMP('2025-01-01') AS ordered_at,
UNION ALL
SELECT
  2 AS id,
  'ice cream' AS product,
  2.99 AS price,
  TIMESTAMP('2025-01-02') AS ordered_at,
UNION ALL
SELECT
  3 AS id,
  'latte' AS product,
  3.99 AS price,
  TIMESTAMP('2025-01-03') AS ordered_at

