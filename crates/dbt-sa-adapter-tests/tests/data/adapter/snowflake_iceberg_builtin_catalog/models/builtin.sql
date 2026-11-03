{{
  config(
    materialized="table",
    catalog_name='builtin_test',
  )
}}

select 'snorlax' as pokemon, 'rest' as move
