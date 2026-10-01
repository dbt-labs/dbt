{%- macro create_backup(relation) -%}
  -- get the standard backup name
  {% set backup_relation = make_backup_relation(relation, relation.type) %}

  -- drop any pre-existing backup
  {{ drop_relation_if_exists(backup_relation) }}

  {{ adapter.rename_relation(relation, backup_relation) }}
{%- endmacro -%}
