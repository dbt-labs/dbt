-- Test that reproduces the exact issue from the GitHub issue
{% macro mymacro() %}
  {% do log(run_query('
    select 1;
    /* end comment */
  '), info=true) %}
  {{ return("GitHub issue test passed!") }}
{% endmacro %}

-- Comprehensive test to investigate dialect differences with comment-only statements
{% macro investigate_dialect_comment_handling() %}
  {{ log("=== Investigating how this dialect handles comment-only statements ===", info=true) }}
  
  -- Test 1: Query with trailing comment (the original issue)
  {{ log("Test 1: Original issue - query with trailing comment", info=true) }}
  {% set result = run_query('select 1 as test_value; /* trailing comment */') %}
  {{ log("  Result: " ~ result, info=true) }}
  
  -- Test 2: Query with line comment after semicolon
  {{ log("Test 2: Query with trailing line comment", info=true) }}
  {% set result = run_query('select 2 as test_value; -- line comment') %}
  {{ log("  Result: " ~ result, info=true) }}
  
  -- Test 3: Query with multiple trailing comments
  {{ log("Test 3: Query with multiple trailing comments", info=true) }}
  {% set result = run_query('select 3 as test_value; /* block comment */ -- line comment') %}
  {{ log("  Result: " ~ result, info=true) }}
  
  -- Test 4: The exact original failing case from GitHub issue  
  {{ log("Test 4: Exact original GitHub issue case", info=true) }}
  {% set result = run_query('select 1; /* end comment */') %}
  {{ log("  Result: " ~ result, info=true) }}
  
  -- Test 5: Comment at start, then SQL statement
  {{ log("Test 5: Comment at start, then SQL", info=true) }}
  {% set result = run_query('/* initial comment */ select 5 as test_value') %}
  {{ log("  Result: " ~ result, info=true) }}
  
  -- Test 6: SQL statement surrounded by comments (should NOT be filtered)
  {{ log("Test 6: SQL surrounded by comments", info=true) }}
  {% set result = run_query('/* before */ select 6 as test_value /* after */') %}
  {{ log("  Result: " ~ result, info=true) }}
  
  -- Test 7: Multiple statements with mixed comments
  {{ log("Test 7: Multiple statements with mixed comments", info=true) }}
  {% set result = run_query('/* start */ select 7 as first; select 8 as second /* end */') %}
  {{ log("  Result: " ~ result, info=true) }}
  
  {{ log("=== All trailing comment tests completed successfully! ===", info=true) }}
  
  {{ return("") }}
{% endmacro %}

-- Test that run_query works with various trailing comment patterns
{% macro test_trailing_comments() %}
  -- Test 1: Simple query with block comment after semicolon
  {% set result1 = run_query('select 1 as num; /* trailing block comment */') %}
  {{ log("Test 1 result: " ~ result1.columns[0].values()[0], info=true) }}
  
  -- Test 2: Query with line comment after semicolon
  {% set result2 = run_query('select 2 as num; -- trailing line comment') %}
  {{ log("Test 2 result: " ~ result2.columns[0].values()[0], info=true) }}
  
  -- Test 3: Query with multiple trailing comments
  {% set result3 = run_query('
    select 3 as num;
    /* block comment */
    -- line comment
  ') %}
  {{ log("Test 3 result: " ~ result3.columns[0].values()[0], info=true) }}
  
  -- Test 4: Query with multiple separate comments
  {% set result4 = run_query('
    select 4 as num;
    /* first comment */ /* second comment */
  ') %}
  {{ log("Test 4 result: " ~ result4.columns[0].values()[0], info=true) }}
  
  -- Test 5: Query without trailing semicolon but with comment
  {% set result5 = run_query('
    select 5 as num
    /* no semicolon, just comment */
  ') %}
  {{ log("Test 5 result: " ~ result5.columns[0].values()[0], info=true) }}
  
  -- Test 6: Comment before SQL (should execute SQL, not filter)
  {% set result6 = run_query('/* leading comment */ select 6 as num') %}
  {{ log("Test 6 result: " ~ result6.columns[0].values()[0], info=true) }}
  
  -- Test 7: SQL surrounded by comments (should execute SQL, not filter)
  {% set result7 = run_query('/* before */ select 7 as num /* after */') %}
  {{ log("Test 7 result: " ~ result7.columns[0].values()[0], info=true) }}
  
  -- Test 8: Multiple statements with comments between (should execute both)
  {% set result8 = run_query('select 8 as first; /* middle comment */ select 88 as second') %}
  {{ log("Test 8 result: " ~ result8.columns[0].values()[0], info=true) }}
  
  {{ return("") }}
{% endmacro %}