{% materialization table, adapter='redis_adbc', supported_languages=['sql', 'python'] %}

  {%- set language = model['language'] -%}
  {%- set existing_relation = load_cached_relation(this) -%}
  {%- set target_relation = this.incorporate(type='table') %}
  {%- set intermediate_relation =  make_intermediate_relation(target_relation) -%}
  {%- set preexisting_intermediate_relation = load_cached_relation(intermediate_relation) -%}
  {%- set backup_relation_type = 'table' if existing_relation is none else existing_relation.type -%}
  {%- set backup_relation = make_backup_relation(target_relation, backup_relation_type) -%}
  {%- set preexisting_backup_relation = load_cached_relation(backup_relation) -%}
  {% set grant_config = config.get('grants') %}

  {{ drop_relation_if_exists(preexisting_intermediate_relation) }}
  {{ drop_relation_if_exists(preexisting_backup_relation) }}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}

  -- `BEGIN` happens here:
  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  -- build model
  {#- py_write_table directly, not through dbt's create_table_as, which
      would indent the model's first line. -#}
  {% call statement('main', language=language) -%}
    {%- if language == 'python' -%}
      {{- redis_adbc__py_write_table(False, intermediate_relation, compiled_code) -}}
    {%- else -%}
      {{ get_create_table_as_sql(False, intermediate_relation, sql) }}
    {%- endif %}
  {%- endcall %}

  {% do create_indexes(intermediate_relation) %}

  -- cleanup
  {% if existing_relation is not none %}
    {% set existing_relation = load_cached_relation(existing_relation) %}
    {% if existing_relation is not none %}
        {{ adapter.rename_relation(existing_relation, backup_relation) }}
    {% endif %}
  {% endif %}

  {{ adapter.rename_relation(intermediate_relation, target_relation) }}

  {{ run_hooks(post_hooks, inside_transaction=True) }}

  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode=True) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  -- `COMMIT` happens here
  {{ adapter.commit() }}

  {{ drop_relation_if_exists(backup_relation) }}

  {{ run_hooks(post_hooks, inside_transaction=False) }}

  {{ return({'relations': [target_relation]}) }}
{% endmaterialization %}


{% macro redis_adbc__get_incremental_microbatch_sql(arg_dict) %}
  {%- set target = arg_dict["target_relation"] -%}
  {%- set source = arg_dict["temp_relation"] -%}
  {%- set dest_columns = arg_dict["dest_columns"] -%}
  {%- set predicates = (arg_dict.get("incremental_predicates") or []) | list -%}
  {%- set event_time = model.config.event_time -%}
  {%- if model.batch and model.batch.event_time_start -%}
    {%- do predicates.append("DBT_INTERNAL_TARGET." ~ event_time ~ " >= '" ~ model.batch.event_time_start ~ "'") -%}
  {%- endif -%}
  {%- if model.batch and model.batch.event_time_end -%}
    {%- do predicates.append("DBT_INTERNAL_TARGET." ~ event_time ~ " < '" ~ model.batch.event_time_end ~ "'") -%}
  {%- endif -%}
  {%- if not predicates -%}
    {{ exceptions.raise_compiler_error("microbatch: no batch window for " ~ target ~ "; refusing to delete every row") }}
  {%- endif -%}
  {%- set cols = get_quoted_csv(dest_columns | map(attribute="name")) %}
  delete from {{ target }} as DBT_INTERNAL_TARGET
  where {{ predicates | join("\n    and ") }};
  insert into {{ target }} ({{ cols }})
  (
    select {{ cols }} from {{ source }}
  )
{% endmacro %}
