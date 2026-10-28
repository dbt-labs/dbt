{{ config(materialized='table') }}

select cast(1 as FLOAT64) as test_float
