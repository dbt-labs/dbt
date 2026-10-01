{% materialization incremental, adapter='spark', supported_languages=['sql', 'python'] -%}
  {#-- Validate early so we don't run SQL if the file_format + strategy combo is invalid --#}
  {%- set raw_file_format = config.get('file_format', default='parquet') -%}
  {%- set raw_strategy = config.get('incremental_strategy') or 'append' -%}
  {%- set grant_config = config.get('grants') -%}

  {%- set file_format = dbt_spark_validate_get_file_format(raw_file_format) -%}
  {%- set strategy = dbt_spark_validate_get_incremental_strategy(raw_strategy, file_format) -%}

  {#-- Set vars --#}

  {%- set unique_key = config.get('unique_key', none) -%}
  {%- set partition_by = config.get('partition_by', none) -%}
  {%- set language = model['language'] -%}
  {%- set on_schema_change = incremental_validate_on_schema_change(config.get('on_schema_change'), default='ignore') -%}
  {%- set incremental_predicates = config.get('predicates', none) or config.get('incremental_predicates', none) -%}
  {%- set target_relation = this -%}
  {%- set existing_relation = load_relation(this) -%}
  {% set tmp_relation = this.incorporate(path = {"identifier": this.identifier ~ '__dbt_tmp'}) -%}

  {#-- for SQL model we will create temp view that doesn't have database and schema --#}
  {%- if language == 'sql'-%}
    {%- set tmp_relation = tmp_relation.include(database=false, schema=false) -%}
  {%- endif -%}

  {#-- Set Overwrite Mode --#}
  {%- if strategy in ['insert_overwrite', 'microbatch'] and partition_by -%}
    {%- call statement() -%}
      set spark.sql.sources.partitionOverwriteMode = DYNAMIC
    {%- endcall -%}
  {%- endif -%}

  {#-- Run pre-hooks --#}
  {{ run_hooks(pre_hooks) }}

  {#-- Incremental run logic --#}
  {%- if existing_relation is none -%}
    {#-- Relation must be created --#}
    {%- call statement('main', language=language) -%}
      {{ create_table_as(False, target_relation, compiled_code, language) }}
    {%- endcall -%}
    {% do persist_constraints(target_relation, model) %}
  {%- elif existing_relation.is_view or should_full_refresh() -%}
    {#-- Relation must be dropped & recreated --#}
    {% set is_delta = (file_format == 'delta' and existing_relation.is_delta) %}
    {% if not is_delta %} {#-- If Delta, we will `create or replace` below, so no need to drop --#}
      {% do adapter.drop_relation(existing_relation) %}
    {% endif %}
    {%- call statement('main', language=language) -%}
      {{ create_table_as(False, target_relation, compiled_code, language) }}
    {%- endcall -%}
    {% do persist_constraints(target_relation, model) %}
  {%- else -%}
    {#-- Relation must be merged --#}
    {%- call statement('create_tmp_relation', language=language) -%}
      {{ create_table_as(True, tmp_relation, compiled_code, language) }}
    {%- endcall -%}
    {%- do process_schema_changes(on_schema_change, tmp_relation, existing_relation) -%}
    {%- call statement('main') -%}
      {{ dbt_spark_get_incremental_sql(strategy, tmp_relation, target_relation, existing_relation, unique_key, incremental_predicates) }}
    {%- endcall -%}
    {%- if language == 'python' -%}
      {#--
      This is yucky.
      See note in dbt-spark/dbt/include/spark/macros/adapters.sql
      re: python models and temporary views.

      Also, why do neither drop_relation or adapter.drop_relation work here?!
      --#}
      {% call statement('drop_relation') -%}
        drop table if exists {{ tmp_relation }}
      {%- endcall %}
    {%- endif -%}
  {%- endif -%}

  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode) %}
  {% do apply_grants(target_relation, grant_config, should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  {{ run_hooks(post_hooks) }}

  {{ return({'relations': [target_relation]}) }}

{%- endmaterialization %}
