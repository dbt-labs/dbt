{{
    config(
        materialized='incremental',
        incremental_strategy='delete+insert',
        unique_key=['id', 'region'],
        file_format='delta'
    )
}}

select id, region, value
from {{ ref('source_data') }}
