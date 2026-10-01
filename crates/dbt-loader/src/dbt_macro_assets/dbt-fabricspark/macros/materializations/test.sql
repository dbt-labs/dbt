{% macro fabricspark__get_test_sql(main_sql, fail_calc, warn_if, error_if, limit) -%}
    select
      INT({{ fail_calc }}) as failures,
      {{ fail_calc }} {{ warn_if }} as should_warn,
      {{ fail_calc }} {{ error_if }} as should_error
    from (
      {{ main_sql }}
      {{ "limit " ~ limit if limit != none }}
    ) dbt_internal_test
{%- endmacro %}