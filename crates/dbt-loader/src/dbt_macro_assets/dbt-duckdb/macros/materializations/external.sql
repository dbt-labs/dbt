{% materialization external, adapter="duckdb", supported_languages=['sql', 'python'] %}

  {%- set location = render(config.get('location', default=external_location(this, config))) -%}
  {%- set rendered_options = render_write_options(config) -%}

  {%- set format = config.get('format') -%}
  {%- set allowed_formats = ['csv', 'parquet', 'json'] -%}
  {%- if format -%}
      {%- if format not in allowed_formats -%}
          {{ exceptions.raise_compiler_error("Invalid format: " ~ format ~ ". Allowed formats are: " ~ allowed_formats | join(', ')) }}
      {%- endif -%}
  {%- else -%}
    {%- set format = location.split('.')[-1].lower() if '.' in location else 'parquet' -%}
    {%- set format = format if format in allowed_formats else 'parquet' -%}
  {%- endif -%}

  {%- set write_options = adapter.external_write_options(location, rendered_options) -%}
  {%- set read_location = adapter.external_read_location(location, rendered_options) -%}
  {%- set parquet_read_options = config.get('parquet_read_options', {'union_by_name': False}) -%}
  {%- set json_read_options = config.get('json_read_options', {'auto_detect': True}) -%}
  {%- set csv_read_options = config.get('csv_read_options', {'auto_detect': True}) -%}

  -- set language - python or sql
  {%- set language = model['language'] -%}

  {%- set target_relation = this.incorporate(type='view') %}

  -- Continue as normal materialization
  {%- set existing_relation = load_cached_relation(this) -%}
  {%- set temp_relation =  make_intermediate_relation(this.incorporate(type='table'), suffix='__dbt_tmp') -%}
  {%- set intermediate_relation =  make_intermediate_relation(target_relation, suffix='__dbt_int') -%}
  -- the intermediate_relation should not already exist in the database; get_relation
  -- will return None in that case. Otherwise, we get a relation that we can drop
  -- later, before we try to use this name for the current operation
  {%- set preexisting_temp_relation = load_cached_relation(temp_relation) -%}
  {%- set preexisting_intermediate_relation = load_cached_relation(intermediate_relation) -%}
  /*
      See ../view/view.sql for more information about this relation.
  */
  {%- set backup_relation_type = 'table' if existing_relation is none else existing_relation.type -%}
  {%- set backup_relation = make_backup_relation(target_relation, backup_relation_type) -%}
  -- as above, the backup_relation should not already exist
  {%- set preexisting_backup_relation = load_cached_relation(backup_relation) -%}
  -- grab current tables grants config for comparision later on
  {% set grant_config = config.get('grants') %}

  -- drop the temp relations if they exist already in the database
  {{ drop_relation_if_exists(preexisting_intermediate_relation) }}
  {{ drop_relation_if_exists(preexisting_temp_relation) }}
  {{ drop_relation_if_exists(preexisting_backup_relation) }}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}

  -- `BEGIN` happens here:
  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  -- build model
  {% call statement('create_table', language=language) -%}
    {{- create_table_as(False, temp_relation, compiled_code, language) }}
  {%- endcall %}

  -- check if relation is empty
  {%- set count_query -%}
    select count(*) as row_count from {{ temp_relation }}
  {%- endset -%}
  {%- set row_count = run_query(count_query) -%}

  -- if relation is empty, write a non-empty table with column names and null values
  {% call statement('main', language='sql') -%}
    {% if row_count[0][0] == 0 %}
    insert into {{ temp_relation }} values (
      {%- for col in get_columns_in_relation(temp_relation) -%}
      NULL,
      {%- endfor -%}
    )
    {% endif %}
  {%- endcall %}

  -- write a temp relation into file
  {{ write_to_file(temp_relation, location, write_options) }}

-- create a view on top of the location
  {% call statement('main', language='sql') -%}
    {% if format == 'json' %}
      create or replace view {{ intermediate_relation }} as (
        select * from read_json('{{ read_location }}'
        {%- for key, value in json_read_options.items() -%}
          , {{ key }}=
          {%- if value is string -%}
            '{{ value }}'
          {%- else -%}
            {{ value }}
          {%- endif -%}
        {%- endfor -%}
        )
        -- if relation is empty, filter by all columns having null values
        {% if row_count[0][0] == 0 %}
          where 1
          {%- for col in get_columns_in_relation(temp_relation) -%}
            {{ '' }} AND "{{ col.column }}" is not NULL
          {%- endfor -%}
        {% endif %}
      );
    {% elif format == 'parquet' %}
      create or replace view {{ intermediate_relation }} as (
        select * from read_parquet('{{ read_location }}'
        {%- for key, value in parquet_read_options.items() -%}
          , {{ key }}=
          {%- if value is string -%}
            '{{ value }}'
          {%- else -%}
            {{ value }}
          {%- endif -%}
        {%- endfor -%}
        )
        -- if relation is empty, filter by all columns having null values
        {% if row_count[0][0] == 0 %}
          where 1
          {%- for col in get_columns_in_relation(temp_relation) -%}
            {{ '' }} AND "{{ col.column }}" is not NULL
          {%- endfor -%}
        {% endif %}
      );
    {% elif format == 'csv' %}
    create or replace view {{ intermediate_relation }} as (
      select * from read_csv('{{ read_location }}'
      {%- for key, value in csv_read_options.items() -%}
        , {{ key }}=
        {%- if value is string -%}
          '{{ value }}'
        {%- else -%}
          {{ value }}
        {%- endif -%}
      {%- endfor -%}
      )
      -- if relation is empty, filter by all columns having null values
      {% if row_count[0][0] == 0 %}
        where 1
        {%- for col in get_columns_in_relation(temp_relation) -%}
          {{ '' }} AND "{{ col.column }}" is not NULL
        {%- endfor -%}
      {% endif %}
    );
    {% endif %}
  {%- endcall %}

  -- cleanup
  {% if existing_relation is not none %}
      {{ adapter.rename_relation(existing_relation, backup_relation) }}
  {% endif %}

  {{ adapter.rename_relation(intermediate_relation, target_relation) }}

  {{ run_hooks(post_hooks, inside_transaction=True) }}

  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode=True) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  -- `COMMIT` happens here
  {{ adapter.commit() }}

  -- finally, drop the existing/backup relation after the commit
  {{ drop_relation_if_exists(backup_relation) }}
  {{ drop_relation_if_exists(temp_relation) }}

  -- register table into glue
  {%- set plugin_name = config.get('plugin') -%}
  {%- set glue_register = config.get('glue_register', default=false) -%}
  {%- set partition_columns = config.get('partition_columns', []) -%}
  {% if plugin_name is not none or glue_register is true %}
    {% if glue_register %}
      {# legacy hack to set the glue database name, deprecate this #}
      {%- set plugin_name = 'glue|' ~ config.get('glue_database', 'default') -%}
    {% endif %}
    {% do store_relation(plugin_name, target_relation, location, format, config) %}
  {% endif %}

  {{ run_hooks(post_hooks, inside_transaction=False) }}

  {{ return({'relations': [target_relation]}) }}

{% endmaterialization %}
