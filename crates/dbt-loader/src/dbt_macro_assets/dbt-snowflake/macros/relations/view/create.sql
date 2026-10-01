{% macro snowflake__create_view_as_with_temp_flag(relation, sql, is_temporary=False) -%}
  {%- set secure = config.get('secure', default=false) -%}
  {%- set copy_grants = config.get('copy_grants', default=false) -%}
  {%- set row_access_policy = config.get('row_access_policy', default=none) -%}
  {%- set table_tag = config.get('table_tag', default=none) -%}
  {%- set sql_header = config.get('sql_header', none) -%}

  {{ sql_header if sql_header is not none }}
  create or replace {% if secure -%}
    secure
  {%- endif %} {% if is_temporary -%}
    temporary
  {%- endif %} view {{ relation }}
  {% if config.persist_column_docs() -%}
    {% set model_columns = model.columns %}
    {% set query_columns = get_columns_in_query(sql) %}
    {{ get_persist_docs_column_list(model_columns, query_columns) }}

  {%- endif %}
  {%- set contract_config = config.get('contract') -%}
  {%- if contract_config.enforced -%}
    {{ get_assert_columns_equivalent(sql) }}
  {%- endif %}
  {% if copy_grants -%} copy grants {%- endif %}
  {% if row_access_policy -%} with row access policy {{ row_access_policy }} {%- endif %}
  {% if table_tag -%} with tag ({{ table_tag }}) {%- endif %}
  as (
    {{ sql }}
  );
{% endmacro %}


{% macro snowflake__create_view_as(relation, sql) -%}
  {{ snowflake__create_view_as_with_temp_flag(relation, sql) }}
{% endmacro %}


/* {#
Vendored from dbt-core for the purpose of overwriting small pieces to support dynamics tables. This should
eventually be retired in favor of a standardized approach. Changed line:

{%- if old_relation is not none and old_relation.is_table -%}  ->
{%- if old_relation is not none and not old_relation.is_view -%}
#} */

{% macro snowflake__create_or_replace_view() %}
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
  {%- if old_relation is not none and not old_relation.is_view -%}
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
