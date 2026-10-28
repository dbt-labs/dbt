-- add_array.sql
SELECT *, ARRAY(1, 2, 3) AS my_array
FROM {{ ref("my_model") }}
