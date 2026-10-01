-- funcsign: (strategy, optional[string], string) -> string
{% macro dbt_snowflake_get_tmp_relation_type(strategy, unique_key, language) %}
{%- set tmp_relation_type = config.get('tmp_relation_type') -%}
  /* {#
       High-level principles:
       If we are running multiple statements (DELETE + INSERT),
       and we want to guarantee identical inputs to both statements,
       then we must first save the model query results as a temporary table
       (which presumably comes with a performance cost).
       If we are running a single statement (MERGE or INSERT alone),
       we _may_ save the model query definition as a view instead,
       for (presumably) faster overall incremental processing.

       Low-level specifics:
       If an invalid option is specified, then we will raise an
       exception with a corresponding message.

       Languages other than SQL (like Python) will use a temporary table.
       With the default strategy of merge, the user may choose between a
       temporary table and view (defaulting to view).

       The append strategy can use a view because it will run a single INSERT
       statement.

       When unique_key is none, the delete+insert and microbatch strategies
       can use a view because a single INSERT statement is run with no DELETES
       as part of the statement. Otherwise, play it safe by using a table.

       Catalog-linked databases (Iceberg tables) do not support temporary
       relations or transient tables — only Iceberg tables are allowed. A
       permanent table is used as the tmp relation for CLD models.

       'transient' is also available as a user-facing tmp_relation_type for
       non-Iceberg models. Unlike session-scoped temporary tables, transient
       tables are visible to Snowflake's lineage tracking. Note that transient
       tables share the regular schema namespace; use the
       snowflake__resolve_incremental_tmp_relation dispatch macro to redirect
       tmp relations to a dedicated schema to avoid name collisions when
       multiple runs share the same target schema.
  #} */

  {% if language == "python" and tmp_relation_type is not none and tmp_relation_type != "table" %}
    {% do exceptions.raise_compiler_error(
      "Python models currently only support 'table' for tmp_relation_type but "
       ~ tmp_relation_type ~ " was specified."
    ) %}
  {% endif %}

  {#-- Python always uses a temporary table, regardless of other conditions --#}
  {% if language != "sql" %}
    {{ return("table") }}
  {% endif %}

  {#-- CLD schemas only support Iceberg tables; use table (not transient) --#}
  {% if snowflake__is_catalog_linked_database(relation=config.model) %}
    {{ return("table") }}
  {% endif %}

  {% if strategy in ["delete+insert", "microbatch"] and tmp_relation_type is not none and tmp_relation_type not in ("table", "transient") and unique_key is not none %}
    {% do exceptions.raise_compiler_error(
      "In order to maintain consistent results when `unique_key` is not none,
      the `" ~ strategy ~ "` strategy only supports `table` or `transient` for `tmp_relation_type` but "
      ~ tmp_relation_type ~ " was specified."
      )
  %}
  {% endif %}

  {% if tmp_relation_type == "table" %}
    {{ return("table") }}
  {% elif tmp_relation_type == "view" %}
    {{ return("view") }}
  {% elif tmp_relation_type == "transient" %}
    {{ return("transient") }}
  {% elif strategy in ("default", "merge", "append", "insert_overwrite") %}
    {{ return("view") }}
  {% elif strategy in ["delete+insert", "microbatch"] and unique_key is none %}
    {{ return("view") }}
  {% else %}
    {{ return("table") }}
  {% endif %}
{% endmacro %}


{#--
  Snowflake-specific despite the unprefixed name. Adheres to upstream:
  https://github.com/dbt-labs/dbt-adapters/blob/d074e17ed3b12164c77d1aa99f869d954848615b/dbt-snowflake/src/dbt/include/snowflake/macros/materializations/incremental.sql#L83
--#}
{% macro resolve_incremental_tmp_relation(tmp_relation) %}
  {{ return(adapter.dispatch('resolve_incremental_tmp_relation', 'dbt')(tmp_relation)) }}
{% endmacro %}


{% macro snowflake__resolve_incremental_tmp_relation(tmp_relation) %}
  {#--
    Override this macro in your project to control where the incremental
    tmp relation is created. Useful for redirecting to a dedicated scratch
    schema to avoid name collisions when multiple runs share the same
    target schema.

    Example:
      {% macro snowflake__resolve_incremental_tmp_relation(tmp_relation) %}
        {{ return(tmp_relation.incorporate(schema='scratch')) }}
      {% endmacro %}
  --#}
  {{ return(tmp_relation) }}
{% endmacro %}


{% materialization incremental, adapter='snowflake', supported_languages=['sql', 'python'] -%}

  {% set original_query_tag = set_query_tag() %}

  {#-- Set vars --#}
  {%- set full_refresh_mode = (should_full_refresh()) -%}
  {%- set language = model['language'] -%}

  {%- set identifier = this.name -%}
  {%- set catalog_relation = adapter.build_catalog_relation(config.model) -%}

  {%- set is_catalog_linked_db = snowflake__is_catalog_linked_database(relation=none, catalog_relation=catalog_relation) -%}

  {%- set target_relation = api.Relation.create(
	identifier=identifier,
	schema=schema,
	database=database,
	type='table',
	table_format=catalog_relation.table_format,
  ) -%}
  {# DIVERGENCE: catalog=config.model.catalog is a metadata hack I found another solution to using build_catalog_relation #}

  {% set existing_relation = load_relation(this) %}

  {#-- The temp relation will be a view, temp table, or transient table, depending on strategy and config --#}
  {%- set unique_key = config.get('unique_key') -%}
  {% set incremental_strategy = config.get('incremental_strategy') or 'default' %}
  {% set tmp_relation_type = dbt_snowflake_get_tmp_relation_type(incremental_strategy, unique_key, language) %}

  {% if is_catalog_linked_db %}
    {% set tmp_relation = make_temp_relation(this).incorporate(type=tmp_relation_type, catalog=catalog_relation.catalog_name, is_table=true) %}
  {% else %}
    {#-- Transient tables are dropped with DROP TABLE, so the relation type must be 'table' --#}
    {% set tmp_relation_object_type = 'table' if tmp_relation_type == 'transient' else tmp_relation_type %}
    {% set tmp_relation = make_temp_relation(this).incorporate(type=tmp_relation_object_type) %}
  {% endif %}
  {% set tmp_relation = resolve_incremental_tmp_relation(tmp_relation) %}

  {% set grant_config = config.get('grants') %}

  {% set on_schema_change = incremental_validate_on_schema_change(config.get('on_schema_change'), default='ignore') %}

  {{ run_hooks(pre_hooks) }}

  {% if existing_relation is none %}
    {%- call statement('main', language=language) -%}
      {{ create_table_as(False, target_relation, compiled_code, language) }}
    {%- endcall -%}

  {% elif existing_relation.is_view %}
    {#-- Can't overwrite a view with a table - we must drop --#}
    {{ log("Dropping relation " ~ target_relation ~ " because it is a view and this model is a table.") }}
    {% do adapter.drop_relation(existing_relation) %}
    {%- call statement('main', language=language) -%}
      {{ create_table_as(False, target_relation, compiled_code, language) }}
    {%- endcall -%}

  {% elif full_refresh_mode %}
    {% if target_relation.needs_to_drop(existing_relation) %}
      {{ drop_relation_if_exists(existing_relation) }}
    {% endif %}
    {%- call statement('main', language=language) -%}
      {{ create_table_as(False, target_relation, compiled_code, language) }}
    {%- endcall -%}

  {% elif target_relation.table_format != existing_relation.table_format %}
    {% do exceptions.raise_compiler_error(
        "Unable to update the incremental model `" ~ target_relation.identifier ~ "` from `" ~ existing_relation.table_format ~ "` to `" ~ target_relation.table_format ~ "` due to Snowflake limitation. Please execute with --full-refresh to drop the table and recreate in the new catalog.'"
      )
    %}

  {% else %}
    {#-- Create the temp relation as a view, temp table, or transient table --#}
    {% if is_catalog_linked_db %}
        {%- call statement('create_tmp_relation', language=language) -%}
          {{ create_table_as(False, tmp_relation, compiled_code, language) }}
        {%- endcall -%}
    {% elif tmp_relation_type == 'view' %}
        {%- call statement('create_tmp_relation') -%}
          {{ snowflake__create_view_as_with_temp_flag(tmp_relation, compiled_code, True) }}
        {%- endcall -%}
    {% elif tmp_relation_type == 'transient' %}
        {%- call statement('create_tmp_relation', language=language) -%}
          {{ snowflake__create_table_transient_sql(tmp_relation, compiled_code) }}
        {%- endcall -%}
    {% else %}
        {%- call statement('create_tmp_relation', language=language) -%}
          {{ create_table_as(True, tmp_relation, compiled_code, language) }}
        {%- endcall -%}
    {% endif %}

    {% do adapter.expand_target_column_types(
           from_relation=tmp_relation,
           to_relation=target_relation) %}
    {#-- Process schema changes. Returns dict of changes if successful. Use source columns for upserting/merging --#}
    {% set dest_columns = process_schema_changes(on_schema_change, tmp_relation, existing_relation) %}
    {% if not dest_columns %}
      {% set dest_columns = adapter.get_columns_in_relation(existing_relation) %}
    {% endif %}

    {#-- Get the incremental_strategy, the macro to use for the strategy, and build the sql --#}
    {% set incremental_predicates = config.get('predicates', none) or config.get('incremental_predicates', none) %}
    {% set strategy_sql_macro_func = adapter.get_incremental_strategy_macro(context, incremental_strategy) %}
    {# DIVERGENCE: pass catalog_relation through to strategy macros #}
    {% set strategy_arg_dict = ({'target_relation': target_relation, 'temp_relation': tmp_relation, 'unique_key': unique_key, 'dest_columns': dest_columns, 'incremental_predicates': incremental_predicates, 'catalog_relation': catalog_relation }) %}

    {%- call statement('main') -%}
      {{ strategy_sql_macro_func(strategy_arg_dict) }}
    {%- endcall -%}
  {% endif %}


  {% do drop_relation_if_exists(tmp_relation) %}

  {{ run_hooks(post_hooks) }}

  {% set target_relation = target_relation.incorporate(type='table') %}

  {# DIVERGENCE: sqlfluff lint suppression #}
  {% set should_revoke =
   should_revoke(existing_relation.is_table, full_refresh_mode) %} -- noqa: existing_relation can be none
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  {% do unset_query_tag(original_query_tag) %}

  {{ return({'relations': [target_relation]}) }}

{%- endmaterialization %}


{% macro snowflake__get_incremental_default_sql(arg_dict) %}
  {{ return(get_incremental_merge_sql(arg_dict)) }}
{% endmacro %}
