{% macro sqlserver__get_test_sql(main_sql, fail_calc, warn_if, error_if, limit) -%}

  -- Create target schema if it does not exist
  {{ get_use_database_sql(target.database) }}
  {{ create_schema_if_not_exists(target.schema) }}

  {% set testview_name = "testview_" ~ local_md5(main_sql) ~ "_" ~ (range(1300, 19000) | random) %}
  {% set testview %}
    {{ adapter.quote(target.schema) }}.{{ adapter.quote(testview_name) }}
  {% endset %}

  {% set sql = main_sql.replace("'", "''")%}
  EXEC('create view {{testview}} as {{ sql }};')
  select
    {{ "top (" ~ limit ~ ')' if limit != none }}
    {{ fail_calc }} as failures,
    case when {{ fail_calc }} {{ warn_if }}
      then 'true' else 'false' end as should_warn,
    case when {{ fail_calc }} {{ error_if }}
      then 'true' else 'false' end as should_error
  from (
    select * from {{testview}}
  ) dbt_internal_test;

  EXEC('drop view {{testview}};')

{%- endmacro %}
