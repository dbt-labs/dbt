{{ config(materialized='streaming_table', alias=('masked_st_prod' if var('phase', 'ci') == 'prod' else 'masked_st')) }}
select * from stream {{ ref('column_mask_seed') }}
