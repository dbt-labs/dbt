{%- materialization view, adapter='singlestore' -%}

  {%- set existing_relation = load_cached_relation(this) -%}
  {%- set target_relation = this.incorporate(type='view') -%}

  {% set grant_config = config.get('grants') %}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}

  -- `BEGIN` happens here:
  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  {% if existing_relation is not none and existing_relation.is_view %}
    -- build model (in-place atomic update: preserves original view if new SQL is invalid)
    {% call statement('main') -%}
      {{ singlestore__alter_view_as(target_relation, sql) }}
    {%- endcall %}
  {% else %}
    {% if existing_relation is not none %}
      {{ adapter.drop_relation(existing_relation) }}
    {% else %}
      {% call statement('drop_view_if_exists') -%}
        drop view if exists {{ target_relation.render() }}
      {%- endcall %}
    {% endif %}

    -- build model
    {% call statement('main') -%}
      {{ get_create_view_as_sql(target_relation, sql) }}
    {%- endcall %}
  {% endif %}

  -- cleanup
  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode=True) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  {{ run_hooks(post_hooks, inside_transaction=True) }}

  {{ adapter.commit() }}

  {{ run_hooks(post_hooks, inside_transaction=False) }}

  {{ return({'relations': [target_relation]}) }}

{%- endmaterialization -%}
