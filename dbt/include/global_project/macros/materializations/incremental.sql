{% macro dbt__incremental_delete(schema, model) -%}

  {%- set unique_key = model['config'].get('unique_key') -%}
  {%- set identifier = model['name'] -%}

  delete
  from "{{ schema }}"."{{ identifier }}"
  where ({{ unique_key }}) in (
    select ({{ unique_key }})
    from "{{ identifier }}__dbt_incremental_tmp"
  );

{%- endmacro %}

{% materialization incremental, adapter='base'-%}

  {%- set identifier = model['name'] -%}
  {%- set tmp_identifier = model['name'] + '__dbt_incremental_tmp' -%}

  {%- set sql_where = model['config'].get('sql_where', 'null') -%}
  {%- set unique_key = model['config'].get('unique_key', 'null') -%}

  {%- set non_destructive_mode = (flags.NON_DESTRUCTIVE == True) -%}
  {%- set full_refresh_mode = (flags.FULL_REFRESH == True) -%}
  {%- set existing = adapter.query_for_existing(schema) -%}
  {%- set existing_type = existing.get(identifier) -%}

  {%- set exists_as_table = (existing_type == 'table') -%}
  {%- set should_truncate = (non_destructive_mode and full_refresh_mode and exists_as_table) -%}
  {%- set should_drop = (not should_truncate and (full_refresh_mode or (existing_type not in (none, 'table')))) -%}
  {%- set force_create = (flags.FULL_REFRESH and not flags.NON_DESTRUCTIVE) -%}

  -- setup
  {% if existing_type is none -%}
    -- noop
  {%- elif should_truncate -%}
    {{ adapter.truncate(identifier) }}
  {%- elif should_drop -%}
    {{ adapter.drop(identifier, existing_type) }}
  {%- endif %}

  {% for hook in pre_hooks %}
    {% statement %}
      {{ hook }};
    {% endstatement %}
  {% endfor %}

  -- build model
  {% if force_create or not adapter.already_exists(schema, identifier) -%}
    {%- statement capture_result -%}
      create table "{{ schema }}"."{{ identifier }}" {{ dist }} {{ sort }} as (
        {{ sql }}
      );
    {%- endstatement -%}
  {%- else -%}
    {%- statement -%}
      create temporary table "{{ tmp_identifier }}" as (
        with dbt_incr_sbq as (
          {{ sql }}
        )
        select * from dbt_incr_sbq
        where ({{ sql_where }})
          or ({{ sql_where }}) is null
        );
     {%- endstatement -%}

     {{ adapter.expand_target_column_types(temp_table=tmp_identifier,
                                           to_schema=schema,
                                           to_table=identifier) }}

     {%- statement capture_result -%}
       {% set dest_columns = adapter.get_columns_in_table(schema, identifier) %}
       {% set dest_cols_csv = dest_columns | map(attribute='quoted') | join(', ') %}

       {% if model.get('config', {}).get('unique_key') is not none -%}

         {{ dbt__incremental_delete(schema, model) }}

       {%- endif %}

       insert into "{{ schema }}"."{{ identifier }}" ({{ dest_cols_csv }})
       (
         select {{ dest_cols_csv }}
         from "{{ identifier }}__dbt_incremental_tmp"
       );
     {% endstatement %}
  {%- endif %}

  {% for hook in post_hooks %}
    {% statement %}
      {{ hook }};
    {% endstatement %}
  {% endfor %}

  {{ adapter.commit() }}

{%- endmaterialization %}
