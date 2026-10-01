{% materialization table, adapter='fabric' %}

  -- Load target relation
  {%- set target_relation = this.incorporate(type='table') %}
  {%- set existing_relation = adapter.get_relation(database=this.database, schema=this.schema, identifier=this.identifier) -%}

  {#-- Drop the relation if it was a view to "convert" it in a table. This may lead to
    -- downtime, but it should be a relatively infrequent occurrence  #}
  {% if existing_relation is not none and not existing_relation.is_table %}
    {{ log("Dropping relation " ~ existing_relation ~ " because it is of type " ~ existing_relation.type) }}
    {{ adapter.drop_relation(existing_relation) }}
  {% endif %}

  -- grab current tables grants config for comparision later on
  {% set grant_config = config.get('grants') %}

  -- Making a temp relation
  {% set temp_relation = make_temp_relation(target_relation, '__dbt_temp') %}

  -- Drop temp relation if it exists before materializing temp relation
  {{ adapter.drop_relation(temp_relation) }}

  {% set tmp_vw_relation = temp_relation.incorporate(path={"identifier": temp_relation.identifier ~ '__dbt_tmp_vw'}, type='view')-%}

  {{ adapter.drop_relation(tmp_vw_relation) }}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}
  -- `BEGIN` happens here:
  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  -- build model
  {% call statement('main') -%}
    {{ create_table_as(False, temp_relation, sql) }}
  {% endcall %}

  {% if existing_relation is not none and existing_relation.is_table %}

    -- making a backup relation, this will come in use when contract is enforced or not
    {%- set set_backup_relation = adapter.get_relation(database=this.database, schema=this.schema, identifier=this.identifier) -%}
    {% if (set_backup_relation != none and set_backup_relation.type == "table") %}
      {%- set backup_relation = make_backup_relation(target_relation, 'table') -%}
    {% elif (set_backup_relation != none and set_backup_relation.type == "view") %}
        {%- set backup_relation = make_backup_relation(target_relation, 'view') -%}
    {% endif %}

    -- Dropping a temp relation if it exists
    {{ adapter.drop_relation(backup_relation) }}

    -- Rename existing relation to back up relation
    {{ adapter.rename_relation(existing_relation, backup_relation) }}

    -- Renaming temp relation as main relation
    {{ adapter.rename_relation(temp_relation, target_relation) }}

    -- Drop backup relation
    {{ adapter.drop_relation(backup_relation) }}

  {%- else %}

      -- Renaming temp relation as main relation
      {{ adapter.rename_relation(temp_relation, target_relation) }}

  {% endif %}

  {{ adapter.drop_relation(tmp_vw_relation) }}

  -- cleanup
  {{ run_hooks(post_hooks, inside_transaction=True) }}

  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}
  {% do persist_docs(target_relation, model) %}
  -- `COMMIT` happens here
  {{ adapter.commit() }}

  -- Add constraints including FK relation.
  {{ build_model_constraints(target_relation) }}
  {{ run_hooks(post_hooks, inside_transaction=False) }}
  {{ return({'relations': [target_relation]}) }}

{% endmaterialization %}
