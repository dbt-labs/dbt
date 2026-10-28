{{
    config(materialized='view')
}}

select * from {{ ref('model_a') }}