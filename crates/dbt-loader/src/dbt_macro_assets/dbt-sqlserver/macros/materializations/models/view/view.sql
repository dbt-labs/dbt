{%- materialization view, adapter='sqlserver' -%}
  {%- set existing_relation = load_cached_relation(this) -%}
  {%- set target_relation = this.incorporate(type='view') -%}
  {%- set intermediate_relation =  make_intermediate_relation(target_relation) -%}

  -- the intermediate_relation should not already exist in the database; get_relation
  -- will return None in that case. Otherwise, we get a relation that we can drop
  -- later, before we try to use this name for the current operation
  {%- set preexisting_intermediate_relation = load_cached_relation(intermediate_relation) -%}
  /*
     This relation (probably) doesn't exist yet. If it does exist, it's a leftover from
     a previous run, and we're going to try to drop it immediately. At the end of this
     materialization, we're going to rename the "existing_relation" to this identifier,
     and then we're going to drop it. In order to make sure we run the correct one of:
       - drop view ...
       - drop table ...

     We need to set the type of this relation to be the type of the existing_relation, if it exists,
     or else "view" as a sane default if it does not. Note that if the existing_relation does not
     exist, then there is nothing to move out of the way and subsequentally drop. In that case,
     this relation will be effectively unused.
  */
  {%- set backup_relation_type = 'view' if existing_relation is none else existing_relation.type -%}
  {%- set backup_relation = make_backup_relation(target_relation, backup_relation_type) -%}
  -- as above, the backup_relation should not already exist
  {%- set preexisting_backup_relation = load_cached_relation(backup_relation) -%}
  -- grab current tables grants config for comparison later on
  {% set grant_config = config.get('grants') %}
  {% set preserved_grants = {} %}
  {% set should_skip_view_update = false %}
  {% set build_sql = none %}

  {% if existing_relation is not none and existing_relation.type != 'view' %}
    {% set current_grants_table = run_query(get_show_grant_sql(existing_relation)) %}
    {% set current_grants_dict = adapter.standardize_grants_dict(current_grants_table) %}
    {% set preserved_grants = {} %}
    {% for privilege, grantees in diff_of_two_dicts(current_grants_dict, grant_config).items() %}
      {% if privilege | lower in ['select', 'insert', 'update', 'delete'] %}
        {% do preserved_grants.update({privilege: grantees}) %}
      {% endif %}
    {% endfor %}
    {% set build_sql = get_create_view_as_sql(intermediate_relation, sql) %}
  {% elif existing_relation is not none and existing_relation.type == 'view' %}
    {% set current_view_definition_table = run_query(get_view_definition_sql(existing_relation)) %}
    {% if current_view_definition_table is not none and current_view_definition_table.rows | length > 0 %}
      {% set normalized_relation = target_relation.include(database=False) | lower | replace('\n', '') | replace('\r', '') | replace('\t', '') | replace(' ', '') | replace(';', '') %}
      {% set normalized_sql = sql | lower | replace('\n', '') | replace('\r', '') | replace('\t', '') | replace(' ', '') | replace(';', '') %}
      {% set normalized_definition = current_view_definition_table.rows[0][0] | lower | replace('\n', '') | replace('\r', '') | replace('\t', '') | replace(' ', '') | replace(';', '') %}
      {% set should_skip_view_update = normalized_definition.endswith(normalized_sql) %}
    {% endif %}
    {% if should_skip_view_update %}
      {% set build_sql = 'declare @dbt_sqlserver_noop int;' %}
    {% else %}
      {% set build_sql = get_create_view_as_sql(target_relation, sql) %}
    {% endif %}
  {% else %}
    {% set build_sql = get_create_view_as_sql(target_relation, sql) %}
  {% endif %}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}

  -- drop the temp relations if they exist already in the database
  {{ drop_relation_if_exists(preexisting_intermediate_relation) }}
  {{ drop_relation_if_exists(preexisting_backup_relation) }}

  -- `BEGIN` happens here:
  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  {% if existing_relation is not none and existing_relation.type != 'view' %}
    -- build model
    {% call statement('main') -%}
      {{ build_sql }}
    {%- endcall %}

    -- cleanup
    -- move the existing relation out of the way
    {% set existing_relation = load_cached_relation(existing_relation) %}
    {% if existing_relation is not none %}
        {{ adapter.rename_relation(existing_relation, backup_relation) }}
    {% endif %}

    {{ adapter.rename_relation(intermediate_relation, target_relation) }}
  {% else %}
    -- build model
    {% call statement('main') -%}
      {{ build_sql }}
    {%- endcall %}
  {% endif %}

  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode=True) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% if preserved_grants %}
    {% do apply_grants(target_relation, preserved_grants, should_revoke=False) %}
  {% endif %}

  {% do persist_docs(target_relation, model) %}

  {{ run_hooks(post_hooks, inside_transaction=True) }}

  {{ adapter.commit() }}

  {{ drop_relation_if_exists(backup_relation) }}

  {{ run_hooks(post_hooks, inside_transaction=False) }}

  {{ return({'relations': [target_relation]}) }}

{%- endmaterialization -%}
