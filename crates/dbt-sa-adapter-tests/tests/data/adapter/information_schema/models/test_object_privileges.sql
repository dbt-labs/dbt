-- Test OBJECT_PRIVILEGES view for BigQuery (requires project-level with region)
{{ setup_test_table() }}

-- Use adapter.get_relation to fetch the relation with location from BigQuery
{% set test_relation = adapter.get_relation(database=target.database, schema=target.schema, identifier='info_schema_test_table') %}

{% if adapter.type == 'bigquery' %}
  {{ log("Found test relation: " ~ test_relation, info=true) }}

  -- For OBJECT_PRIVILEGES, emulate python: incorporate location first
  {% set location = adapter.get_dataset_location(test_relation) %}
  {% set test_relation = test_relation.incorporate(location=location) %}

  -- Generate INFORMATION_SCHEMA.OBJECT_PRIVILEGES path (project-level with region)
  {% set object_privileges_info_schema = test_relation.information_schema("OBJECT_PRIVILEGES") %}
  
  {{ log("Testing INFORMATION_SCHEMA.OBJECT_PRIVILEGES access:", info=true) }}
  {{ log("  OBJECT_PRIVILEGES view: " ~ object_privileges_info_schema, info=true) }}
  
  -- Query INFORMATION_SCHEMA.OBJECT_PRIVILEGES
  select 
      object_catalog,
      object_schema,
      object_name,
      object_type,
      privilege_type,
      grantee,
      'Listed from ' || '{{ object_privileges_info_schema }}' as source
  from {{ object_privileges_info_schema }}
  where object_schema = '{{ target.schema }}'
    and object_name = 'info_schema_test_table'
  limit 10
{% else %}
  select 'SKIPPED - object privileges not supported for this adapter' as result
{% endif %}
