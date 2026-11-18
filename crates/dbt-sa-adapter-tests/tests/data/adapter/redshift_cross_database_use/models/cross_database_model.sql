{{ config(materialized='table', database='ci_alt') }}

select 1 as id
