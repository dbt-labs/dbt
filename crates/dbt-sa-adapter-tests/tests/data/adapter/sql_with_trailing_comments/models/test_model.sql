{{ config(materialized='table') }}

-- This model tests that SQL with trailing comments compiles and runs correctly
{{ test_trailing_comments() }}

select 
  1 as test_id,
  'sql_with_trailing_comments' as test_name
/* End of query with trailing comment */
