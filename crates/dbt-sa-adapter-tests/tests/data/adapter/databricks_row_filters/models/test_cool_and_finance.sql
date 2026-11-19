-- depends on {{ function('hide_cool_and_finance') }}
select * from {{ ref('base_model_seed') }}