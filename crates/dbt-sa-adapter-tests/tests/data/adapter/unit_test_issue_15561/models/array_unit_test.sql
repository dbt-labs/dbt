SELECT
    user_id,
    name,
    hobbies,
    ARRAY_LENGTH(hobbies) AS hobby_count,
    test_scores,
    ARRAY_LENGTH(test_scores) AS test_count,
    (SELECT AVG(score) FROM UNNEST(test_scores) AS score) AS avg_test_score,
    activity_dates,
    ARRAY_LENGTH(activity_dates) AS activity_count,
    (SELECT MAX(date) FROM UNNEST(activity_dates) AS date) AS latest_activity_date,
    ARRAY_CONCAT(
        hobbies,
        [CONCAT("avg_score:", CAST(ROUND((SELECT AVG(score) FROM UNNEST(test_scores) AS score), 1) AS STRING))]
    ) AS summary_info
FROM {{ ref('stg_array_unit_test') }}
