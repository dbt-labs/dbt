{% materialization incremental, adapter="duckdb", supported_languages=['sql', 'python'] -%}

  {%- set language = model['language'] -%}
  -- only create temp tables if using local duckdb, as it is not currently supported for remote databases
  {# DIVERGENCE: Fusion uses adapter.has_feature("motherduck"); v1 dbt-duckdb exposes
     adapter.is_motherduck. Gate on dbt_version. #}
  {%- if dbt_version.startswith('2.') -%}
    {%- set temporary = not adapter.has_feature("motherduck") -%}
  {%- else -%}
    {%- set temporary = not adapter.is_motherduck() -%}
  {%- endif -%}

  -- relations
  {%- set existing_relation = load_cached_relation(this) -%}
  {%- set target_relation = this.incorporate(type='table') -%}
  {%- set temp_relation = make_temp_relation(target_relation)-%}
  {%- set intermediate_relation = make_intermediate_relation(target_relation)-%}
  {%- set backup_relation_type = 'table' if existing_relation is none else existing_relation.type -%}
  {%- set backup_relation = make_backup_relation(target_relation, backup_relation_type) -%}

  -- configs
  {%- set unique_key = config.get('unique_key') -%}
  {%- set full_refresh_mode = (should_full_refresh()  or existing_relation.is_view) -%}
  {%- set on_schema_change = incremental_validate_on_schema_change(config.get('on_schema_change'), default='ignore') -%}

  -- the temp_ and backup_ relations should not already exist in the database; get_relation
  -- will return None in that case. Otherwise, we get a relation that we can drop
  -- later, before we try to use this name for the current operation. This has to happen before
  -- BEGIN, in a separate transaction
  {%- set preexisting_intermediate_relation = load_cached_relation(intermediate_relation)-%}
  {%- set preexisting_backup_relation = load_cached_relation(backup_relation) -%}
   -- grab current tables grants config for comparision later on
  {% set grant_config = config.get('grants') %}
  {{ drop_relation_if_exists(preexisting_intermediate_relation) }}
  {{ drop_relation_if_exists(preexisting_backup_relation) }}

  {% set to_drop = [] %}
  {% if not temporary %}
    -- if not using a temporary table we will update the temp relation to use a different temp schema ("dbt_temp" by default)
    -- for microbatch with concurrent batches, include batch timestamps in the identifier to avoid collisions
    {%- set batch_id = '' -%}
    {%- set batch_ctx = model.get('batch') -%}
    {%- if batch_ctx and batch_ctx.get('event_time_start') -%}
        {%- set batch_id = batch_ctx.get('event_time_start') | string | replace('-', '') | replace(':', '') | replace(' ', '_') | replace('+', '') -%}
    {%- endif -%}
    {% set temp_relation = temp_relation.incorporate(path=adapter.get_temp_relation_path(this, batch_id)) %}
    {% do run_query(create_schema(temp_relation)) %}
    {# DIVERGENCE: Fusion uses adapter.has_feature("transactions"); v1 dbt-duckdb exposes
       adapter.disable_transactions (note inverted polarity). Gate on dbt_version. #}
    {% if (dbt_version.startswith('2.') and adapter.has_feature("transactions"))
          or (not dbt_version.startswith('2.') and not adapter.disable_transactions()) %}
      {% do adapter.commit() %}
    {% endif %}
    -- then drop the temp relation after we insert the incremental data into the target relation
    {% do to_drop.append(temp_relation) %}
  {% endif %}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}
  -- `BEGIN` happens here:
  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  {% if existing_relation is none %}
    {% set build_sql = create_table_as(False, target_relation, compiled_code, language) %}
  {% elif full_refresh_mode %}
    {% set build_sql = create_table_as(False, intermediate_relation, compiled_code, language) %}
    {% set need_swap = true %}
  {% else %}
    {% if language == 'python' %}
      {% set build_python = create_table_as(temporary, temp_relation, compiled_code, language) %}
      {% call statement("pre", language=language) %}
        {{- build_python }}
      {% endcall %}
    {% endif %}

    {% set dest_columns = process_schema_changes(on_schema_change, temp_relation, existing_relation) %}

    {% if not dest_columns %}
      {% set dest_columns = adapter.get_columns_in_relation(existing_relation) %}
    {% endif %}

    {#-- Get the incremental_strategy, the macro to use for the strategy, and build the sql --#}
    {% set incremental_strategy = config.get('incremental_strategy') or 'default' %}
    {% set incremental_predicates = config.get('predicates', none) or config.get('incremental_predicates', none) %}
    {% set strategy_sql_macro_func = adapter.get_incremental_strategy_macro(context, incremental_strategy) %}
    {% set strategy_arg_dict = ({'target_relation': target_relation, 'temp_relation': temp_relation, 'unique_key': unique_key, 'dest_columns': dest_columns, 'incremental_predicates': incremental_predicates }) %}
    {% set build_sql = strategy_sql_macro_func(strategy_arg_dict) %}
    {% set language = "sql" %}

    {#-- Emit CREATE TEMP TABLE + strategy as ONE multi-statement batch.
         DuckDB does not split on ';', so the entire batch runs in a single execute() call
         on one connection, ensuring the temp table is visible to the subsequent DML. --#}
    {% set create_tmp_sql = create_table_as(temporary, temp_relation, compiled_code) %}
    {% set build_sql = create_tmp_sql ~ ";\n" ~ build_sql %}

  {% endif %}

  {% call statement("main", language=language) %}
      {{- build_sql }}
  {% endcall %}

  {% if need_swap %}
      {#-- Drop indexes on target relation before renaming to backup to avoid dependency errors --#}
      {% do drop_indexes_on_relation(target_relation) %}
      {% do adapter.rename_relation(target_relation, backup_relation) %}
      {% do adapter.rename_relation(intermediate_relation, target_relation) %}
      {% do to_drop.append(backup_relation) %}
  {% endif %}

  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {# Align order with table materialization to avoid MotherDuck alter conflicts #}
  {% if existing_relation is none or existing_relation.is_view or should_full_refresh() %}
    {% do create_indexes(target_relation) %}
  {% endif %}

  {% do persist_docs(target_relation, model) %}

  {{ run_hooks(post_hooks, inside_transaction=True) }}

  -- `COMMIT` happens here
  {% do adapter.commit() %}

  {% for rel in to_drop %}
      {# On MotherDuck the temp relation is a real table; dropping it cascades indexes. Avoid extra ALTERs.
         DIVERGENCE: Fusion uses adapter.has_feature("motherduck"); v1 uses adapter.is_motherduck. #}
      {% set _is_motherduck = adapter.has_feature("motherduck") if dbt_version.startswith('2.') else adapter.is_motherduck() %}
      {% if not _is_motherduck %}
        {% do drop_indexes_on_relation(rel) %}
      {% endif %}
      {% do adapter.drop_relation(rel) %}
  {% endfor %}

  {{ run_hooks(post_hooks, inside_transaction=False) }}

  {{ return({'relations': [target_relation]}) }}

{%- endmaterialization %}