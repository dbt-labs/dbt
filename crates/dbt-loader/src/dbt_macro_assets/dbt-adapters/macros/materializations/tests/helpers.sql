-- funcsign: (string, string, string, string, optional[integer]) -> string
{% macro get_test_sql(main_sql, fail_calc, warn_if, error_if, limit) -%}
  {{ adapter.dispatch('get_test_sql', 'dbt')(main_sql, fail_calc, warn_if, error_if, limit) }}
{%- endmacro %}

-- funcsign: (string, string, string, string, optional[integer]) -> string
{% macro default__get_test_sql(main_sql, fail_calc, warn_if, error_if, limit) -%}
    select
      {{ fail_calc }} as failures,
      {{ fail_calc }} {{ warn_if }} as should_warn,
      {{ fail_calc }} {{ error_if }} as should_error
    from (
      {{ main_sql }}
      {{ "limit " ~ limit if limit != none }}
    ) dbt_internal_test
{%- endmacro %}




-- funcsign: (string, string, list[string]) -> string
{% macro get_unit_test_sql(main_sql, expected_fixture_sql, expected_column_names) -%}
  {{ adapter.dispatch('get_unit_test_sql', 'dbt')(main_sql, expected_fixture_sql, expected_column_names) }}
{%- endmacro %}

-- funcsign: (string, string, list[string]) -> string
{% macro default__get_unit_test_sql(main_sql, expected_fixture_sql, expected_column_names) -%}
-- Build actual result given inputs
with dbt_internal_unit_test_actual as (
  select
    {% for expected_column_name in expected_column_names %}{{expected_column_name}}{% if not loop.last -%},{% endif %}{%- endfor -%}, {{ dbt.string_literal("actual") }} as {{ adapter.quote("actual_or_expected") }}
  from (
    {{ main_sql }}
  ) _dbt_internal_unit_test_actual
),
-- Build expected result
dbt_internal_unit_test_expected as (
  select
    {% for expected_column_name in expected_column_names %}{{expected_column_name}}{% if not loop.last -%}, {% endif %}{%- endfor -%}, {{ dbt.string_literal("expected") }} as {{ adapter.quote("actual_or_expected") }}
  from (
    {{ expected_fixture_sql }}
  ) _dbt_internal_unit_test_expected
)
-- Union actual and expected results
select * from dbt_internal_unit_test_actual
union all
select * from dbt_internal_unit_test_expected
{%- endmacro %}
