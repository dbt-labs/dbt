{{ config(materialized='incremental', incremental_strategy='merge', unique_key='id') }}

{% if is_incremental() %}
select 2 as id
{% else %}
select 1 as id
{% endif %}
