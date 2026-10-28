{{
  config(
    materialized="table",
    catalog_name='builtin_test',
    adapter_properties={
      'base_location_root': '_fusion_ci/props',
      'base_location_subpath': 'subpath_props',
    }
  )
}}

select 'snorlax' as pokemon, 'rest' as move
