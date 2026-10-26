{{ config(
    materialized='table'
) }}
SELECT
    1 AS id,
    [
        STRUCT(7 AS d, 21 AS e, 5 AS b_id, "color" AS a_color),
        STRUCT(11 AS d, 13 AS e, 3 AS b_id, "different color" AS a_color)
    ] AS array_of_structs,
    STRUCT(1 as b, 2 as a, [STRUCT(1 as d, 2 as c)] AS array_of_structs) AS just_struct,
    STRUCT(STRUCT(1 as b, 2 as a) as b, STRUCT(3 as d, 4 as c) as a) AS nested_struct