{%- materialization test, default -%}

  {% set relations = [] %}
  {% set limit = config.get('limit') %}
  {# DIVERGENCE: FIXME: support REQUIRE_SQL_HEADER_IN_TEST_CONFIGS flag #}
  {% set sql_header = config.get('sql_header') if flags.REQUIRE_SQL_HEADER_IN_TEST_CONFIGS else none %}

  {% set sql_with_limit %}
    {{ get_limit_subquery_sql(sql, limit) }}
  {% endset %}

  {% if should_store_failures() %}

    {% set identifier = model['alias'] %}
    {% set old_relation = adapter.get_relation(database=database, schema=schema, identifier=identifier) %}

    {% set store_failures_as = config.get('store_failures_as') %}
    -- if `--store-failures` is invoked via command line and `store_failures_as` is not set,
    -- config.get('store_failures_as', 'table') returns None, not 'table'
    {% if store_failures_as == none %}{% set store_failures_as = 'table' %}{% endif %}
    {% if store_failures_as not in ['table', 'view'] %}
        {{ exceptions.raise_compiler_error(
            "'" ~ store_failures_as ~ "' is not a valid value for `store_failures_as`. "
            "Accepted values are: ['ephemeral', 'table', 'view']"
        ) }}
    {% endif %}

    {% set target_relation = api.Relation.create(
        identifier=identifier, schema=schema, database=database, type=store_failures_as) -%} %}

    {% if old_relation %}
        {% do adapter.drop_relation(old_relation) %}
    {% endif %}

    {% call statement(auto_begin=True) %}
        {% if sql_header %}{{ sql_header }}{% endif %}
        {{ get_create_sql(target_relation, sql_with_limit) }}
    {% endcall %}

    {% do relations.append(target_relation) %}

    {# Since the test failures have already been saved to the database, reuse that result rather than querying again #}
    {% set main_sql %}
        select *
        from {{ target_relation }}
    {% endset %}

    {{ adapter.commit() }}

  {% else %}

      {% set main_sql = sql_with_limit %}

  {% endif %}

  {% set fail_calc = config.get('fail_calc') %}
  {% set warn_if = config.get('warn_if') %}
  {% set error_if = config.get('error_if') %}

  {% call statement('main', fetch_result=True) -%}

    {% if sql_header %}{{ sql_header }}{% endif %}
    {# The limit has already been included above, and we do not want to duplicate it again. We also want to be safe for macro overrides treating `limit` as a required parameter. #}
    {{ get_test_sql(main_sql, fail_calc, warn_if, error_if, limit=none)}}

  {%- endcall %}

  {{ return({'relations': relations}) }}

{%- endmaterialization -%}
