{{
    config(
        materialized='materialized_view',
    )
}}

select id, value from {{ ref('table_hidden_schema') }}
