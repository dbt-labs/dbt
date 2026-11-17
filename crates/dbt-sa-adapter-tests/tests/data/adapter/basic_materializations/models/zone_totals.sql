{{ config(materialized='view') }}

select zone, sum(trips) as trips
from {{ ref('trips_table') }}
group by zone
