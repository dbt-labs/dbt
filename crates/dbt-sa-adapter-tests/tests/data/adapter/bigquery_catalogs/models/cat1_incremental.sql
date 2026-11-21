{# Test that incremental tables can use catalogs as well #}
{{ config(
    materialized='incremental',
    unique_key='updated_at',
    catalog_name='cat1',
) }}

select
  name,
  id,
  updated_at
from {{ ref('standard_table') }}

{% if is_incremental() %}
  where updated_at > (select max(updated_at) from {{ this }})
{% endif %}

