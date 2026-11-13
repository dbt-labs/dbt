{% macro create_or_replace_view(run_outside_transaction_hooks=True) %}
  {%- set identifier = model['alias'] -%}

  {%- set lf_tags_config = config.get('lf_tags_config') -%}
  {%- set lf_inherited_tags = config.get('lf_inherited_tags') -%}
  {%- set lf_grants = config.get('lf_grants') -%}

  {%- set old_relation = adapter.get_relation(database=database, schema=schema, identifier=identifier) -%}
  {%- set exists_as_view = (old_relation is not none and old_relation.is_view) -%}
  {%- set target_relation = api.Relation.create(
      identifier=identifier,
      schema=schema,
      database=database,
      type='view',
    ) -%}

  {% if run_outside_transaction_hooks %}
      -- no transactions on BigQuery
      {{ run_hooks(pre_hooks, inside_transaction=False) }}
  {% endif %}
  -- `BEGIN` happens here on Snowflake
  {{ run_hooks(pre_hooks, inside_transaction=True) }}
  -- If there's a table with the same name and we weren't told to full refresh,
  -- that's an error. If we were told to full refresh, drop it. This behavior differs
  -- for Snowflake and BigQuery, so multiple dispatch is used.
  {%- if old_relation is not none and old_relation.is_table -%}
    {{ handle_existing_table(should_full_refresh(), old_relation) }}
  {%- endif -%}

  -- build model
  {% call statement('main') -%}
    {{ create_view_as(target_relation, compiled_code) }}
  {%- endcall %}

  {% if lf_tags_config is not none %}
    {{ adapter.add_lf_tags(target_relation, lf_tags_config) }}
  {% endif %}

  {% if lf_grants is not none %}
    {{ adapter.apply_lf_grants(target_relation, lf_grants) }}
  {% endif %}

  {{ run_hooks(post_hooks, inside_transaction=True) }}

  {{ adapter.commit() }}

  {% if run_outside_transaction_hooks %}
      -- No transactions on BigQuery
      {{ run_hooks(post_hooks, inside_transaction=False) }}
  {% endif %}

  {{ return({'relations': [target_relation]}) }}

{% endmacro %}
