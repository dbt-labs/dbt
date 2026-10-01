{% materialization snapshot, adapter='fabric' %}

  {%- set target_table = model.get('alias', model.get('name')) -%}
  {%- set strategy_name = config.get('strategy') -%}
  {%- set unique_key = config.get('unique_key') %}
  -- grab current tables grants config for comparision later on
  {%- set grant_config = config.get('grants') -%}

  {% set target_relation_exists, target_relation = get_or_create_relation(
          database=model.database,
          schema=model.schema,
          identifier=target_table,
          type='table') -%}

  {%- if not target_relation.is_table -%}
    {% do exceptions.relation_wrong_type(target_relation, 'table') %}
  {%- endif -%}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}
  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  {% set strategy_macro = strategy_dispatch(strategy_name) %}
  {% set strategy = strategy_macro(model, "snapshotted_data", "source_data", model['config'], target_relation_exists) %}

  {% set temp_snapshot_relation_exists, temp_snapshot_relation = get_or_create_relation(
          database=model.database,
          schema=model.schema,
          identifier=target_table+"_snapshot_staging_temp_view",
          type='view') -%}

  -- Create a temporary view to manage if user SQl uses CTE
  {% set temp_snapshot_relation_sql = model['compiled_code'] %}
  {{ adapter.drop_relation(temp_snapshot_relation) }}

  {% call statement('create temp_snapshot_relation') -%}
    {{ get_create_view_as_sql(temp_snapshot_relation, temp_snapshot_relation_sql) }}
  {%- endcall %}

  {% if not target_relation_exists %}

      {% set build_sql = build_snapshot_table(strategy, temp_snapshot_relation) %}
      {% set build_or_select_sql = build_sql %}

      -- naming a temp relation
      {% set tmp_relation_view = target_relation.incorporate(path={"identifier": target_relation.identifier ~ '__dbt_tmp_vw'}, type='view')-%}
      -- Fabric & Synapse adapters use temp relation because of lack of CTE support for CTE in CTAS, Insert
      -- drop temp relation if exists
      {{ adapter.drop_relation(tmp_relation_view) }}
      {% set final_sql = get_create_table_as_sql(False, target_relation, build_sql) %}
      {{ adapter.drop_relation(tmp_relation_view) }}

  {% else %}

      {% set columns = config.get("snapshot_table_column_names") or get_snapshot_table_column_names() %}
      {{ adapter.valid_snapshot_target(target_relation, columns) }}
      {% set build_or_select_sql = snapshot_staging_table(strategy, temp_snapshot_relation, target_relation) %}
      {% set staging_table = build_snapshot_staging_table(strategy, temp_snapshot_relation, target_relation) %}
      -- this may no-op if the database does not require column expansion
      {% do adapter.expand_target_column_types(from_relation=staging_table,
                                               to_relation=target_relation) %}

      {% set remove_columns = ['dbt_change_type', 'DBT_CHANGE_TYPE', 'dbt_unique_key', 'DBT_UNIQUE_KEY'] %}
      {% if unique_key | is_list %}
          {% for key in strategy.unique_key %}
              {{ remove_columns.append('dbt_unique_key_' + loop.index|string) }}
              {{ remove_columns.append('DBT_UNIQUE_KEY_' + loop.index|string) }}
          {% endfor %}
      {% endif %}
      {% set missing_columns = adapter.get_missing_columns(staging_table, target_relation)
                                   | rejectattr('name', 'in', remove_columns)
                                   | list %}
      {% if missing_columns|length > 0 %}
        {{log("Missing columns length is: "~ missing_columns|length)}}
        {% do create_columns(target_relation, missing_columns) %}
      {% endif %}
      {% set source_columns = adapter.get_columns_in_relation(staging_table)
                                   | rejectattr('name', 'in', remove_columns)
                                   | list %}
      {% set quoted_source_columns = [] %}
      {% for column in source_columns %}
        {% do quoted_source_columns.append(adapter.quote(column.name)) %}
      {% endfor %}
      {% set final_sql = snapshot_merge_sql(
            target = target_relation,
            source = staging_table,
            insert_cols = quoted_source_columns
         )
      %}
  {% endif %}
  {{ check_time_data_types(build_or_select_sql) }}
  {% call statement('main') %}
      {{ final_sql }}
  {% endcall %}

  {{ adapter.drop_relation(temp_snapshot_relation) }}
  {% set should_revoke = should_revoke(target_relation_exists, full_refresh_mode=False) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  {% if not target_relation_exists %}
    {% do create_indexes(target_relation) %}
  {% endif %}

  {{ run_hooks(post_hooks, inside_transaction=True) }}
  {{ adapter.commit() }}

  {% if staging_table is defined %}
      {% do post_snapshot(staging_table) %}
  {% endif %}

  {{ run_hooks(post_hooks, inside_transaction=False) }}
  {{ return({'relations': [target_relation]}) }}

{% endmaterialization %}
