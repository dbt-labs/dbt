{%- materialization aggregated_test, default -%}

  {% call statement('main', fetch_result=True) -%}
    {{ get_aggregated_test_sql(sql) }}
  {%- endcall %}

  {{ return({'relations': []}) }}

{%- endmaterialization -%}

-- funcsign: (string) -> string
{% macro get_aggregated_test_sql(main_sql) -%}
  {{ adapter.dispatch('get_aggregated_test_sql', 'dbt')(main_sql) }}
{%- endmacro %}

-- funcsign: (string) -> string
{% macro default__get_aggregated_test_sql(main_sql) -%}
    -- Process aggregated test results by column
    -- Expects the main_sql to return a column_name and a failures count per failing column
    with aggregated_data as (
      {{ main_sql }}
    )
    select
      column_name,
      sum(failures) as failures,
      sum(failures) > 0 as should_warn,
      sum(failures) > 0 as should_error
    from aggregated_data
    group by column_name
    order by column_name
{%- endmacro %}
