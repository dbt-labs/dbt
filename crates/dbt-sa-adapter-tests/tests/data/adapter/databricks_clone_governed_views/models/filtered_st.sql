{{ config(materialized='streaming_table', alias=('filtered_st_prod' if var('phase', 'ci') == 'prod' else 'filtered_st')) }}
select * from stream {{ ref('row_filter_seed') }}
