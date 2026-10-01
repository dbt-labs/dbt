WITH
  trailing_2_day_revenue_src AS (
    SELECT f.ordered_at AS src_time, f.amount AS measure_value FROM "main"."orders" AS f
  ),
  trailing_2_day_revenue_spine AS (
    SELECT ds::DATE AS spine_time FROM generate_series((SELECT MIN(src_time) FROM trailing_2_day_revenue_src), (SELECT MAX(src_time) FROM trailing_2_day_revenue_src) + INTERVAL '2 day', INTERVAL '1 day') AS t(ds)
  ),
  trailing_2_day_revenue AS (
    SELECT DATE_TRUNC('day', spine.spine_time) AS metric_time, SUM(src.measure_value) AS trailing_2_day_revenue FROM trailing_2_day_revenue_spine AS spine INNER JOIN trailing_2_day_revenue_src AS src ON src.src_time <= spine.spine_time AND src.src_time > spine.spine_time - INTERVAL '2 day' GROUP BY 1
  )
SELECT *
FROM trailing_2_day_revenue
