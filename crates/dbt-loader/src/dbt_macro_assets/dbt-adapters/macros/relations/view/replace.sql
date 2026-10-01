{% macro get_replace_view_sql(relation, sql) %}
    {{- adapter.dispatch('get_replace_view_sql', 'dbt')(relation, sql) -}}
{% endmacro %}


{% macro default__get_replace_view_sql(relation, sql) %}
    {{ exceptions.raise_compiler_error(
        "`get_replace_view_sql` has not been implemented for this adapter."
    ) }}
{% endmacro %}


/* {#
       Core materialization implementation. BigQuery and Snowflake are similar
       because both can use `create or replace view` where the resulting view's columns
       are not necessarily the same as those of the existing view. On Redshift, this would
       result in: ERROR:  cannot change number of columns in view

       This implementation is superior to the create_temp, swap_with_existing, drop_old
       paradigm because transactions don't run DDL queries atomically on Snowflake. By using
       `create or replace view`, the materialization becomes atomic in nature.
#} */

{% macro create_or_replace_view() %}
  {%- set identifier = model['alias'] -%}

  {%- set old_relation = adapter.get_relation(database=database, schema=schema, identifier=identifier) -%}
  {%- set exists_as_view = (old_relation is not none and old_relation.is_view) -%}

  {%- set target_relation = api.Relation.create(
      identifier=identifier, schema=schema, database=database,
      type='view') -%}
  {% set grant_config = config.get('grants') %}

  {{ run_hooks(pre_hooks) }}

  -- If there's a table with the same name and we weren't told to full refresh,
  -- that's an error. If we were told to full refresh, drop it. This behavior differs
  -- for Snowflake and BigQuery, so multiple dispatch is used.
  {%- if old_relation is not none and old_relation.is_table -%}
    {{ handle_existing_table(should_full_refresh(), old_relation) }}
  {%- endif -%}

  -- build model
  {% call statement('main') -%}
    {{ get_create_view_as_sql(target_relation, sql) }}
  {%- endcall %}

  {% set should_revoke = should_revoke(exists_as_view, full_refresh_mode=True) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {{ run_hooks(post_hooks) }}

  {{ return({'relations': [target_relation]}) }}

{% endmacro %}


{% macro handle_existing_table(full_refresh, old_relation) %}
    {{ adapter.dispatch('handle_existing_table', 'dbt')(full_refresh, old_relation) }}
{% endmacro %}

{% macro default__handle_existing_table(full_refresh, old_relation) %}
    {{ log("Dropping relation " ~ old_relation.render() ~ " because it is of type " ~ old_relation.type) }}
    {{ adapter.drop_relation(old_relation) }}
{% endmacro %}
