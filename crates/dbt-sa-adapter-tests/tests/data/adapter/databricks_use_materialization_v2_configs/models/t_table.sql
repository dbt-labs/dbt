{{ config(materialized='table', use_safer_relation_operations=true) }}
select 1 as id
