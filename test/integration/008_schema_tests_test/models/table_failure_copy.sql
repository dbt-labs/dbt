
{{
    config(
        materialized='table'
    )
}}

select * from {{ this.schema }}.seed_failure
