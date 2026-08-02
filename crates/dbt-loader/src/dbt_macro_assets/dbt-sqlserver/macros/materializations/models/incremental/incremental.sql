{% materialization incremental, adapter='sqlserver' -%}

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
  {%- set full_refresh_build = config.get('full_refresh_build', 'heap_then_index') -%}
  {%- if full_refresh_build == 'prebuilt' -%}
    {{ exceptions.raise_compiler_error(
      "full_refresh_build='prebuilt' is not implemented yet in dbt Core v2's SQL Server "
      "adapter (tracked as a follow-up issue). Use the default 'heap_then_index'."
    ) }}
  {%- elif full_refresh_build != 'heap_then_index' -%}
    {{ exceptions.raise_compiler_error(
      "Invalid full_refresh_build '" ~ full_refresh_build ~ "'. Only 'heap_then_index' (default) is supported."
    ) }}
  {%- endif -%}

  -- the temp_ and backup_ relations should not already exist in the database; get_relation
  -- will return None in that case. Otherwise, we get a relation that we can drop
  -- later, before we try to use this name for the current operation. This has to happen before
  -- BEGIN, in a separate transaction
  {%- set preexisting_intermediate_relation = load_cached_relation(intermediate_relation)-%}
  {%- set preexisting_backup_relation = load_cached_relation(backup_relation) -%}
   -- grab current tables grants config for comparison later on
  {% set grant_config = config.get('grants') %}
  {{ drop_relation_if_exists(preexisting_intermediate_relation) }}
  {{ drop_relation_if_exists(preexisting_backup_relation) }}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}

  -- `BEGIN` happens here:
  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  {% set to_drop = [] %}
  {% set need_swap = false %}
  {#- true only where the statement('main') batch below carries create_table_as
      DDL, i.e. the fresh-create / full-refresh branches. The incremental
      branch's strategy DML stays transactional, so it leaves this false. -#}
  {% set build_sql_is_create_table_as = false %}

  {% if existing_relation is none %}
    {% set build_sql = get_create_table_as_sql(False, target_relation, sql) %}
    {% set build_sql_is_create_table_as = true %}
  {% elif full_refresh_mode %}
    {% set build_sql = get_create_table_as_sql(False, intermediate_relation, sql) %}
    {% set build_sql_is_create_table_as = true %}
    {% set need_swap = true %}
  {% else %}

    {#- The temp build is all catalog DDL (CREATE OR ALTER VIEW / SELECT *
        INTO / DROP VIEW) and must not share the ambient transaction with the
        strategy DML: held to commit, its sysschobjs X keylocks deadlock a
        second worker. Nothing opens a transaction here - run_query never
        auto-begins, and find_references (relation.sql) no longer does either -
        so each statement autocommits and drops its catalog locks as it
        finishes. The strategy DML below still runs transactionally, via
        statement('main')'s default auto_begin through to adapter.commit(). -#}
    {% do run_query(get_create_table_as_sql(True, temp_relation, sql)) %}

    {% set contract_config = config.get('contract') %}
    {% if not contract_config or not contract_config.enforced %}
      {% set expansion_max_rows = config.get('column_type_expansion_max_rows', 1000000) %}
      {% do adapter.expand_target_column_types(
               from_relation=temp_relation,
               to_relation=target_relation,
               max_rows=expansion_max_rows) %}
    {% endif %}
    {#-- Process schema changes. Returns dict of changes if successful. Use source columns for upserting/merging --#}
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

    {% do to_drop.append(temp_relation) %}
  {% endif %}

  {% if build_sql_is_create_table_as %}
    {#- This batch is create_table_as catalog DDL, so letting statement() open
        the ambient transaction would hold its sysschobjs X keylocks until
        adapter.commit() and deadlock a second worker. -#}
    {% call statement("main", auto_begin=False) %}
        {{ build_sql }}
    {% endcall %}
    {#- Reopen the ambient transaction the batch above declined to start, so
        the swap and the tail (grants/persist_docs/indexes/post-hooks) keep
        their semantics and adapter.commit() below has a matching BEGIN
        rather than raising Msg 3902. -#}
    {% do adapter.commit_if_open() %}
    {% do adapter.begin_if_closed() %}
    {{ build_model_constraints(target_relation) }}
  {% else %}
    {% call statement("main") %}
        {{ build_sql }}
    {% endcall %}
  {% endif %}

  {% if need_swap %}
      {% do adapter.rename_relation(target_relation, backup_relation) %}
      {% do adapter.rename_relation(intermediate_relation, target_relation) %}
      {% do to_drop.append(backup_relation) %}
  {% endif %}

  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  {% if build_sql_is_create_table_as %}
    {% do create_indexes(target_relation) %}
  {% endif %}

  {{ run_hooks(post_hooks, inside_transaction=True) }}

  -- `COMMIT` happens here
  {% do adapter.commit() %}

  {% for rel in to_drop %}
      {% do adapter.drop_relation(rel) %}
  {% endfor %}

  {{ run_hooks(post_hooks, inside_transaction=False) }}

  {{ return({'relations': [target_relation]}) }}

{%- endmaterialization %}
