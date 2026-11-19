{{ config(materialized='table') }}

-- First, investigate how this dialect handles comment-only statements
{% do investigate_dialect_comment_handling() %}

-- Then reproduce the exact issue from GitHub #468
{% do log("=== Testing original GitHub issue ===", info=true) %}
{% do run_query('
  select 1;
  /* end comment */
') %}
{% do log("✓ Original issue test completed", info=true) %}

select 'dialect investigation completed' as status
