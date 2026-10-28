{{ config(
    materialized='materialized_view',
    schema='redshift-mv-hyphen-in-fqn'
    )
}}

select * from {{ ref('base') }}
