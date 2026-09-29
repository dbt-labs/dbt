{% materialization distributed_incremental, adapter='clickhouse' %}
  {% set insert_distributed_sync = run_query("SELECT value FROM system.settings WHERE name = 'insert_distributed_sync'")[0][0] %}
  {% if insert_distributed_sync != '1' %}
     {% do exceptions.raise_compiler_error('To use distributed materialization setting insert_distributed_sync should be set to 1') %}
  {% endif %}

  {%- set local_suffix = get_clickhouse_local_suffix() -%}
  {%- set local_db_prefix = get_clickhouse_local_db_prefix() -%}

  {%- set existing_relation = load_cached_relation(this) -%}
  {%- set target_relation = this.incorporate(type='table') -%}

  {% set on_cluster = on_cluster_clause(target_relation) %}
  {% if on_cluster.strip() == '' %}
     {% do exceptions.raise_compiler_error('To use distributed materializations cluster setting in dbt profile must be set') %}
  {% endif %}

  {% set existing_relation_local = load_cached_relation(this.incorporate(path={"identifier": this.identifier + local_suffix, "schema": local_db_prefix + this.schema})) %}
  {% set target_relation_local = target_relation.incorporate(path={"identifier": this.identifier + local_suffix, "schema": local_db_prefix + this.schema}) if target_relation is not none else none %}

  {%- set unique_key = config.get('unique_key') -%}
  {% if unique_key is not none and unique_key|length == 0 %}
    {% set unique_key = none %}
  {% endif %}
  {% if unique_key is iterable and (unique_key is not string and unique_key is not mapping) %}
     {% set unique_key = unique_key|join(', ') %}
  {% endif %}
  {%- set inserts_only = config.get('inserts_only') -%}
  {%- set grant_config = config.get('grants') -%}
  {%- set has_contract = config.get('contract').enforced -%}
  {%- set full_refresh_mode = (should_full_refresh() or existing_relation.is_view) -%}
  {%- set on_schema_change = incremental_validate_on_schema_change(config.get('on_schema_change'), default='ignore') -%}


  {{ create_schema(target_relation_local) }}
  {%- set intermediate_relation = make_intermediate_relation(target_relation_local)-%}
  {%- set distributed_intermediate_relation = make_intermediate_relation(target_relation)-%}
  {%- set backup_relation_type = 'table' if existing_relation is none else existing_relation.type -%}
  {%- set backup_relation = make_backup_relation(target_relation_local, backup_relation_type) -%}
  {%- set distributed_backup_relation = make_backup_relation(target_relation, backup_relation_type) -%}
  {%- set preexisting_intermediate_relation = load_cached_relation(intermediate_relation)-%}
  {%- set preexisting_backup_relation = load_cached_relation(backup_relation) -%}
  {%- set view_relation = default__make_temp_relation(target_relation, '__dbt_view_tmp') -%}

  {{ drop_relation_if_exists(preexisting_intermediate_relation) }}
  {{ drop_relation_if_exists(preexisting_backup_relation) }}
  {{ drop_relation_if_exists(view_relation) }}
  {{ drop_relation_if_exists(distributed_intermediate_relation) }}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}
  {{ run_hooks(pre_hooks, inside_transaction=True) }}
  {% set to_drop = [] %}
  {% set schema_changes = none %}

  {% call statement('main') %}
    {{ create_view_as(view_relation, sql) }}
  {% endcall %}

  {% if existing_relation_local is none %}
    -- No existing local table, recreate local and distributed tables
    {{ create_distributed_local_table(target_relation, target_relation_local, view_relation, sql, has_contract) }}

  {% elif full_refresh_mode %}
    -- Completely replacing the old table, so create a temporary table and then swap it
    {{ create_distributed_local_table(distributed_intermediate_relation, intermediate_relation, view_relation, sql, has_contract) }}
    {% do adapter.drop_relation(distributed_intermediate_relation) or '' %}
    {% set need_swap = true %}

  {% elif inserts_only -%}
    -- There are no updates/deletes or duplicate keys are allowed.  Simply add all of the new rows to the existing
    -- table. It is the user's responsibility to avoid duplicates.  Note that "inserts_only" is a ClickHouse adapter
    -- specific configurable that is used to avoid creating an expensive intermediate table.
    {% call statement('main') %}
        {{ clickhouse__insert_into(target_relation, sql, has_contract) }}
    {% endcall %}

  {% else %}
    {% if existing_relation is none %}
      {% do run_query(create_distributed_table(target_relation, target_relation_local)) %}
      {% set existing_relation = target_relation %}
    {% endif %}

    {% set incremental_strategy = adapter.calculate_incremental_strategy(config.get('incremental_strategy'))  %}
    {% set incremental_predicates = config.get('predicates', []) or config.get('incremental_predicates', []) %}
    {% set partition_by = config.get('partition_by') %}
    {% do adapter.validate_incremental_strategy(incremental_strategy, incremental_predicates, unique_key, partition_by) %}
    {%- if on_schema_change != 'ignore' %}
      {%- set local_column_changes = adapter.check_incremental_schema_changes(on_schema_change, existing_relation_local, sql, query_settings=config.get('query_settings', {})) -%}
      {% if local_column_changes and incremental_strategy != 'legacy' %}
        {% do clickhouse__apply_column_changes(local_column_changes, existing_relation, True) %}
        {% set existing_relation = load_cached_relation(this) %}
      {% endif %}
    {% endif %}
    {% if incremental_strategy == 'legacy' %}
      {% do clickhouse__incremental_legacy(existing_relation, intermediate_relation, local_column_changes, unique_key, True) %}
      {% set need_swap = true %}
    {% elif incremental_strategy == 'delete_insert' %}
      {% do clickhouse__incremental_delete_insert(existing_relation, unique_key, incremental_predicates, True) %}
    {% elif incremental_strategy == 'insert_overwrite' %}
      {% do clickhouse__incremental_insert_overwrite(existing_relation, partition_by, True) %}
    {% elif incremental_strategy == 'append' %}
      {% call statement('main') %}
        {{ clickhouse__insert_into(target_relation, sql, has_contract) }}
      {% endcall %}
    {% endif %}
  {% endif %}

  {% if need_swap %}
      {% if False %}
        {% do adapter.rename_relation(intermediate_relation, backup_relation) %}
        {% do exchange_tables_atomic(backup_relation, target_relation_local) %}
      {% else %}
        {% do adapter.rename_relation(target_relation_local, backup_relation) %}
        {% do adapter.rename_relation(intermediate_relation, target_relation_local) %}
      {% endif %}

      -- Structure could have changed, need to update distributed table from replaced local table
      {% set target_relation_new = target_relation.incorporate(path={"identifier": target_relation.identifier + '_temp'}) %}
      {{ drop_relation_if_exists(target_relation_new) }}
      {% do run_query(create_distributed_table(target_relation_new, target_relation_local)) %}

      {% if False %}
        {% do adapter.rename_relation(target_relation_new, distributed_backup_relation) %}
        {% do exchange_tables_atomic(distributed_backup_relation, target_relation) %}
      {% else %}
        {% do adapter.rename_relation(target_relation, distributed_backup_relation) %}
        {% do adapter.rename_relation(target_relation_new, target_relation) %}
      {% endif %}

      {% do to_drop.append(backup_relation) %}
      {% do to_drop.append(distributed_backup_relation) %}
  {% endif %}

  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}
  {% do apply_grants(target_relation_local, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  {% if existing_relation is none or existing_relation.is_view or should_full_refresh() %}
    {% do create_indexes(target_relation) %}
  {% endif %}

  {{ run_hooks(post_hooks, inside_transaction=True) }}

  {% do adapter.commit() %}

  {% for rel in to_drop %}
      {% do adapter.drop_relation(rel) %}
  {% endfor %}

  {{ run_hooks(post_hooks, inside_transaction=False) }}

  {{ return({'relations': [target_relation]}) }}

{%- endmaterialization %}
