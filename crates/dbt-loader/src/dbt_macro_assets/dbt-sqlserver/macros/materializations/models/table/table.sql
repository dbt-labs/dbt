{% materialization table, adapter='sqlserver' %}

  {%- set existing_relation = load_cached_relation(this) -%}
  {%- set target_relation = this.incorporate(type='table') %}
  {%- set intermediate_relation =  make_intermediate_relation(target_relation) -%}
  -- the intermediate_relation should not already exist in the database; get_relation
  -- will return None in that case. Otherwise, we get a relation that we can drop
  -- later, before we try to use this name for the current operation
  {%- set preexisting_intermediate_relation = load_cached_relation(intermediate_relation) -%}
  /*
      See ../view/view.sql for more information about this relation.
  */
  {%- set backup_relation_type = 'table' if existing_relation is none else existing_relation.type -%}
  {%- set backup_relation = make_backup_relation(target_relation, backup_relation_type) -%}
  -- as above, the backup_relation should not already exist
  {%- set preexisting_backup_relation = load_cached_relation(backup_relation) -%}
  -- grab current tables grants config for comparison later on
  {% set grant_config = config.get('grants') %}

  {%- set table_refresh_method = config.get('table_refresh_method', 'rename') -%}
  {%- if table_refresh_method == 'dml' -%}
    {{ exceptions.raise_compiler_error(
      "table_refresh_method='dml' is not implemented yet in dbt Core v2's SQL Server "
      "adapter (tracked as a follow-up issue). Use the default 'rename'."
    ) }}
  {%- elif table_refresh_method != 'rename' -%}
    {{ exceptions.raise_compiler_error(
      "Invalid table_refresh_method '" ~ table_refresh_method ~ "'. Only 'rename' (default) is supported."
    ) }}
  {%- endif -%}
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

  -- drop the temp relations if they exist already in the database
  {{ drop_relation_if_exists(preexisting_intermediate_relation) }}
  {{ drop_relation_if_exists(preexisting_backup_relation) }}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}

  -- `BEGIN` happens here:
  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  -- build model
  {% call statement('main') -%}
    {{ get_create_table_as_sql(False, intermediate_relation, sql) }}
  {%- endcall %}

  -- cleanup
  {% if existing_relation is not none %}
     /* Do the equivalent of rename_if_exists. 'existing_relation' could have been dropped
        since the variable was first set. */
    {% set existing_relation = load_cached_relation(existing_relation) %}
    {% if existing_relation is not none %}
        {{ adapter.rename_relation(existing_relation, backup_relation) }}
    {% endif %}
  {% endif %}

  {{ adapter.rename_relation(intermediate_relation, target_relation) }}

  {% do create_indexes(target_relation) %}

  {{ run_hooks(post_hooks, inside_transaction=True) }}

  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode=True) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  {{ build_model_constraints(target_relation) }}

  -- `COMMIT` happens here
  {{ adapter.commit() }}

  -- finally, drop the existing/backup relation after the commit
  {{ drop_relation_if_exists(backup_relation) }}

  {{ run_hooks(post_hooks, inside_transaction=False) }}

  {{ return({'relations': [target_relation]}) }}
{% endmaterialization %}
