{{ config(materialized='table') }}
select
{%- for s in range(0, 10) %}
  '2026-11-16 10:49:35.861123456'::timestamp_ntz({{ s }}) as ntz_{{ s }}{{ "," if not loop.last }}
{%- endfor %}
