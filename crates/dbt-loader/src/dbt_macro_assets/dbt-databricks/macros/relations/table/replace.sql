{% macro safe_relation_replace(existing_relation, staging_relation, intermediate_relation, compiled_code) %}
  {{ create_table_at(staging_relation, intermediate_relation, compiled_code) }}

  {{ create_backup(existing_relation) }}

  {{ adapter.rename_relation(staging_relation, existing_relation) }}

  {% call statement('main') %}
    {{ get_drop_backup_sql(existing_relation) }}
  {% endcall %}

  {{ adapter.cache_dropped(make_backup_relation(existing_relation, existing_relation.type)) }}

  {{ rename_staged_key_constraints(staging_relation) }}

  {{ drop_relation_if_exists(intermediate_relation) }}
{% endmacro %}

{% macro rename_staged_key_constraints(staging_relation) %}
  {#- Rename to the configured or model-identifier names the incremental constraint diff expects. -#}
  {% set key_constraints = get_model_key_constraints() %}
  {% set target_relation = this.incorporate(type='table') %}
  {% set staged = staging_relation.enrich(key_constraints).create_constraints %}
  {% set renamed = target_relation.enrich(key_constraints).create_constraints %}

  {#- Drop foreign keys before primary keys and add them after, so a self-referencing FK never outlives its PK.
      IF EXISTS because a staged self-FK references the old table and is removed with its backup. -#}
  {% for key_type in ['foreign_key', 'primary_key'] %}
    {% for constraint in staged if constraint.type == key_type %}
      {% call statement('drop staged constraint') %}
        ALTER TABLE {{ target_relation.render() }} DROP CONSTRAINT IF EXISTS {{ constraint.name }}
      {% endcall %}
    {% endfor %}
  {% endfor %}
  {% for key_type in ['primary_key', 'foreign_key'] %}
    {% for constraint in renamed if constraint.type == key_type %}
      {% call statement('add constraint') %}
        ALTER TABLE {{ target_relation.render() }} ADD {{ constraint.render() }}
      {% endcall %}
    {% endfor %}
  {% endfor %}
{% endmacro %}
