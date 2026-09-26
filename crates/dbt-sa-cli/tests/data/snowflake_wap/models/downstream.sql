{{ config(materialized='table', alias=var('prefix') ~ '_DOWNSTREAM') }}

select id from {{ ref('orders') }}
