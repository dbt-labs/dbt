{{ config(
    materialized="view"
) }}
select * from {{ ref("model") }}