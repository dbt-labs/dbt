{% materialization table_function, adapter='duckdb' %}
  -- This materialization uses DuckDB's Table Function / Table Macro feature to provide parameterized views.
  -- Why use this?
  --     Late binding of functions means that the underlying table can change (have new columns added), and
  --       the function does not need to be recreated. (With a view, the create view statement would need to be re-run).
  --       This allows for skipping parts of the dbt DAG, even if the underlying table changed.
  --     Parameters can force filter pushdown
  --     Functions can provide advanced features like dynamic SQL (the query and query_table functions)

  -- For usage examples, see the tests at /dbt-duckdb/tests/functional/adapter/test_table_function.py
  --     (Don't forget parentheses when you pull from a table_function!)

  -- Using Redshift as an example:
  -- https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-redshift/src/dbt/include/redshift/macros/materializations/table.sql
  {%- set identifier = model['alias'] -%}
  {%- set target_relation = api.Relation.create(
      identifier=identifier,
      schema=schema,
      database=database,
      type='view') -%}
  {%- set backup_relation = none -%}

  -- The parameters config is used to pass in the names of the parameters that will be used within the table function.
  -- parameters can be a single string value (with or without commas), or a list of strings.
  {%- set parameters=config.get('parameters') -%}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}

  -- `BEGIN` happens here:
  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  -- Create or replace the function (macro)
  -- By using create or replace (and a transaction), we do not need an old version and new version.
  {% call statement('main') -%}
    create or replace function {{ target_relation.render() }}(
        {% if not parameters %}
        {% elif parameters is string or parameters is number %}
          {{ parameters if parameters }}
        {% else  %}
          {{ parameters|join(', ') }}
        {% endif %}
      ) as table (
        {{ sql }});
  {%- endcall %}

  {{ run_hooks(post_hooks, inside_transaction=True) }}

  {% do persist_docs(target_relation, model) %}

  -- `COMMIT` happens here:
  {{ adapter.commit() }}

  {{ run_hooks(post_hooks, inside_transaction=False) }}

  {{ return({'relations': [target_relation]}) }}

{% endmaterialization %}
