{{ config(materialized='table') }}

{% do run_query("select 1 as ") %}

select 1 as a
