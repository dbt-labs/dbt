{% materialization table, adapter = 'trino' %}
  {%- set on_table_exists = config.get('on_table_exists', 'rename') -%}
  {% if on_table_exists not in ['rename', 'drop', 'replace', 'skip'] %}
      {%- set log_message = 'Invalid value for on_table_exists (%s) specified. Setting default value (%s).' % (on_table_exists, 'rename') -%}
      {% do log(log_message) %}
      {%- set on_table_exists = 'rename' -%}
  {% endif %}

  {%- set existing_relation = load_cached_relation(this) -%}
  {%- set target_relation = this.incorporate(type='table') %}

  {% if on_table_exists == 'rename' %}
      {%- set intermediate_relation =  make_intermediate_relation(target_relation) -%}
      -- the intermediate_relation should not already exist in the database; get_relation
      -- will return None in that case. Otherwise, we get a relation that we can drop
      -- later, before we try to use this name for the current operation
      {%- set preexisting_intermediate_relation = load_cached_relation(intermediate_relation) -%}

      {%- set backup_relation_type = 'table' if existing_relation is none else existing_relation.type -%}
      {%- set backup_relation = make_backup_relation(target_relation, backup_relation_type) -%}
      -- as above, the backup_relation should not already exist
      {%- set preexisting_backup_relation = load_cached_relation(backup_relation) -%}

      -- drop the temp relations if they exist already in the database
      {{ drop_relation_if_exists(preexisting_intermediate_relation) }}
      {{ drop_relation_if_exists(preexisting_backup_relation) }}
  {% endif %}

  {{ run_hooks(pre_hooks) }}

  -- grab current tables grants config for comparision later on
  {% set grant_config = config.get('grants') %}

  {#-- Create table with given `on_table_exists` mode #}
  {% do on_table_exists_logic(on_table_exists, existing_relation, intermediate_relation, backup_relation, target_relation) %}

  {% do persist_docs(target_relation, model) %}

  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode=True) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {{ run_hooks(post_hooks) }}

  {{ return({'relations': [target_relation]}) }}
{% endmaterialization %}


{% macro on_table_exists_logic(on_table_exists, existing_relation, intermediate_relation, backup_relation, target_relation) -%}
  {#-- Create table with given `on_table_exists` mode #}
  {% if on_table_exists == 'rename' %}

      {#-- table does not exists #}
      {% if existing_relation is none %}
          {% call statement('main') -%}
              {{ create_table_as(False, target_relation, sql) }}
          {%- endcall %}

      {#-- table does exists #}
      {% else %}
          {#-- build modeldock #}
          {% call statement('main') -%}
              {{ create_table_as(False, intermediate_relation, sql) }}
          {%- endcall %}

          {#-- cleanup #}
          {{ adapter.rename_relation(existing_relation, backup_relation) }}
          {{ adapter.rename_relation(intermediate_relation, target_relation) }}

          {#-- finally, drop the existing/backup relation after the commit #}
          {{ drop_relation_if_exists(backup_relation) }}
      {% endif %}

  {% elif on_table_exists == 'drop' %}
      {#-- cleanup #}
      {%- if existing_relation is not none -%}
          {{ adapter.drop_relation(existing_relation) }}
      {%- endif -%}

      {#-- build model #}
      {% call statement('main') -%}
        {{ create_table_as(False, target_relation, sql) }}
      {%- endcall %}

  {% elif on_table_exists == 'replace' %}
      {#-- build model #}
      {% call statement('main') -%}
        {{ create_table_as(False, target_relation, sql, 'replace') }}
      {%- endcall %}

  {% elif on_table_exists == 'skip' %}
      {#-- build model #}
      {% call statement('main') -%}
        {{ create_table_as(False, target_relation, sql, 'skip') }}
      {%- endcall %}

  {% endif %}
{% endmacro %}
