SELECT
    id,
    customer_name,
    total_amount,
    category
FROM {{ ref('serverless_python_model') }}
