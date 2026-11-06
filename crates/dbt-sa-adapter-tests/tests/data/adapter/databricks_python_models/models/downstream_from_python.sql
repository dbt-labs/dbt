SELECT
    id,
    customer_name,
    total_amount,
    category,
    total_amount * 1.1 as total_with_tax
FROM {{ ref('basic_python_model') }}

