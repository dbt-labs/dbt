-- Create a test table  
{{ setup_test_table() }}

{% set test_relation = api.Relation.create(database=target.database, schema=target.schema, identifier='info_schema_test_table') %}

-- Generate INFORMATION_SCHEMA.COLUMNS path using the relation.information_schema() method
{% set columns_info_schema = test_relation.information_schema("COLUMNS") %}

{{ log("Testing INFORMATION_SCHEMA.COLUMNS access:", info=true) }}
{{ log("  COLUMNS view: " ~ columns_info_schema, info=true) }}

-- Query INFORMATION_SCHEMA.COLUMNS to get column details for our test table
select 
    table_catalog,
    table_schema,
    table_name,
    column_name,
    data_type,
    ordinal_position,
    'Listed from ' || '{{ columns_info_schema }}' as source
from {{ columns_info_schema }}
where table_schema = '{{ target.schema }}'
  and upper(table_name) = 'INFO_SCHEMA_TEST_TABLE'
order by ordinal_position
