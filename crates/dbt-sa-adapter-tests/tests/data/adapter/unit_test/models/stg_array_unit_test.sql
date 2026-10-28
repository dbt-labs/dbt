SELECT 
    1 AS user_id,
    "Alice" AS name,
    ["reading", "swimming", "coding"] AS hobbies,
    [85, 92, 78, 95] AS test_scores,
    [CAST('2024-01-15' AS DATE), CAST('2024-02-20' AS DATE), CAST('2024-03-10' AS DATE)] AS activity_dates
UNION ALL
SELECT
    2 AS user_id,
    "Bob" AS name,
    ["gaming", "cooking"] AS hobbies,
    [88, 76, 91] AS test_scores,
    [CAST('2024-01-20' AS DATE), CAST('2024-02-25' AS DATE)] AS activity_dates
UNION ALL
SELECT
    3 AS user_id,
    "Charlie" AS name,
    ["music", "sports", "travel", "photography"] AS hobbies,
    [92, 89, 94, 87, 90] AS test_scores,
    [CAST('2024-01-10' AS DATE), CAST('2024-02-15' AS DATE), CAST('2024-03-05' AS DATE), CAST('2024-03-20' AS DATE)] AS activity_dates
