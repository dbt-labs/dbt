{{
  config(
    materialized="table",
    catalog_name='builtin_test',
    base_location_root='_fusion_ci/model',
    base_location_subpath='subpath_model',
  )
}}

select 'snorlax' as pokemon, 'rest' as move
