{% macro fabric__get_test_sql(main_sql, fail_calc, warn_if, error_if, limit) -%}

  with test_main_sql as (
  {{ main_sql }}
  ),
  dbt_internal_test as (
    select {{ "top (" ~ limit ~ ')' if limit != none }} * from test_main_sql
  )
  select
    {{ fail_calc }} as failures,
    case when {{ fail_calc }} {{ warn_if }}
      then 1 else 0 end as should_warn,
    case when {{ fail_calc }} {{ error_if }}
      then 1 else 0 end as should_error
  from dbt_internal_test

{%- endmacro %}

{% macro fabric__get_unit_test_sql(main_sql, expected_fixture_sql, expected_column_names) -%}
  -- Build actual result given inputs
  WITH dbt_internal_unit_test_actual AS (

    WITH main_sql AS (
      {{ main_sql }}
    )
    SELECT
      {% for expected_column_name in expected_column_names %}
        {{ expected_column_name }}{% if not loop.last -%},{% endif %}
      {%- endfor -%},
      {{ dbt.string_literal("actual") }} AS {{ adapter.quote("actual_or_expected") }}
    FROM main_sql
  ),

  -- Build expected result
  dbt_internal_unit_test_expected AS (

    WITH expected_fixture_sql AS (
      {{ expected_fixture_sql }}
    )
    SELECT
      {% for expected_column_name in expected_column_names %}
        {{ expected_column_name }}{% if not loop.last -%}, {% endif %}
      {%- endfor -%},
      {{ dbt.string_literal("expected") }} AS {{ adapter.quote("actual_or_expected") }}
    FROM expected_fixture_sql
  )

  -- Union actual and expected results
  SELECT * FROM dbt_internal_unit_test_actual
  UNION ALL
  SELECT * FROM dbt_internal_unit_test_expected

{%- endmacro %}
