{{ config(
    materialized='incremental',
    unique_key='id',
    incremental_strategy='merge',
    on_schema_change='sync_all_columns',
    incremental_predicates=['DBT_INTERNAL_DEST.id > 0'],
) }}

select
    id,
    cast(name as varchar(80)) as name,
    cast(100 as decimal(10, 2)) as amount
from {{ ref('source') }}
