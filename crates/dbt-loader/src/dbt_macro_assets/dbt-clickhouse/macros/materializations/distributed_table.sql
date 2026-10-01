{% materialization distributed_table, adapter='clickhouse' %}
  {% set insert_distributed_sync = run_query("SELECT value FROM system.settings WHERE name = 'insert_distributed_sync'")[0][0] %}
  {% if insert_distributed_sync != '1' %}
     {% do exceptions.raise_compiler_error('To use distributed materialization setting insert_distributed_sync should be set to 1') %}
  {% endif %}

  {%- set local_suffix = get_clickhouse_local_suffix() -%}
  {%- set local_db_prefix = get_clickhouse_local_db_prefix() -%}

  {%- set existing_relation = load_cached_relation(this) -%}
  {%- set target_relation = this.incorporate(type='table') -%}

  {% set on_cluster = on_cluster_clause(target_relation) %}
  {% if on_cluster.strip() == '' %}
     {% do exceptions.raise_compiler_error('To use distributed materialization cluster setting in dbt profile must be set') %}
  {% endif %}

  {% set existing_relation_local = existing_relation.incorporate(path={"identifier": this.identifier + local_suffix, "schema": local_db_prefix + this.schema}) if existing_relation is not none else none %}
  {% set target_relation_local = target_relation.incorporate(path={"identifier": this.identifier + local_suffix, "schema": local_db_prefix + this.schema}) if target_relation is not none else none %}

  {%- set backup_relation = none -%}
  {%- set preexisting_backup_relation = none -%}
  {%- set preexisting_intermediate_relation = none -%}

  {% if existing_relation_local is not none %}
    {%- set backup_relation_type = existing_relation_local.type -%}
    {%- set backup_relation = make_backup_relation(target_relation_local, backup_relation_type) -%}
    {%- set preexisting_backup_relation = load_cached_relation(backup_relation) -%}
    {% if not existing_relation.can_exchange %}
      {%- set intermediate_relation =  make_intermediate_relation(target_relation_local) -%}
      {%- set preexisting_intermediate_relation = load_cached_relation(intermediate_relation) -%}
    {% endif %}
  {% endif %}
  {% set view_relation = default__make_temp_relation(target_relation, '__dbt_tmp') %}
  -- drop the temp relations if they exist already in the database
  {{ drop_relation_if_exists(preexisting_intermediate_relation) }}
  {{ drop_relation_if_exists(preexisting_backup_relation) }}
  {{ drop_relation_if_exists(view_relation) }}

  {% set grant_config = config.get('grants') %}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}

  {% call statement('main') %}
    {{ create_view_as(view_relation, sql) }}
  {% endcall %}

  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  {% set has_contract = config.get('contract').enforced %}

  {% if backup_relation is none %}
    {{ create_distributed_local_table(target_relation, target_relation_local, view_relation, none, has_contract) }}
  {% elif existing_relation.can_exchange %}
    -- We can do an atomic exchange, so no need for an intermediate
    {% call statement('main') -%}
      {{ create_empty_table_from_relation(backup_relation, view_relation) }}
    {%- endcall %}
    {% do exchange_tables_atomic(backup_relation, existing_relation_local) %}
  {% else %}
    {% do run_query(create_empty_table_from_relation(intermediate_relation, view_relation)) or '' %}
    {{ adapter.rename_relation(existing_relation_local, backup_relation) }}
    {{ adapter.rename_relation(intermediate_relation, target_relation_local) }}
  {% endif %}
    {% do run_query(create_distributed_table(target_relation, target_relation_local)) or '' %}
  {% do run_query(clickhouse__insert_into(target_relation, sql, has_contract)) or '' %}
  {{ drop_relation_if_exists(view_relation) }}
  -- cleanup
  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode=True) %}
  {% do apply_grants(target_relation_local, grant_config, should_revoke=should_revoke) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}
  {{ run_hooks(post_hooks, inside_transaction=True) }}
  {{ adapter.commit() }}
  {{ drop_relation_if_exists(backup_relation) }}
  {{ run_hooks(post_hooks, inside_transaction=False) }}
  {{ return({'relations': [target_relation]}) }}

{% endmaterialization %}

{% macro create_distributed_table(relation, local_relation) %}
    {%- set cluster = get_clickhouse_cluster_name() -%}
   {% if cluster is none %}
        {% do exceptions.raise_compiler_error('Cluster name should be defined for using distributed materializations, current is None') %}
    {% endif %}

   {%- set cluster = cluster[1:-1] -%}
   {%- set sharding = config.get('sharding_key') -%}

    create or replace table {{ relation }} {{ on_cluster_clause(relation) }} as {{ local_relation }}
    ENGINE = Distributed('{{ cluster}}', '{{ local_relation.schema }}', '{{ local_relation.name }}'
    {%- if sharding is not none and sharding.strip() != '' -%}
        , {{ sharding }}
    {%- else %}
        , rand()
    {% endif -%}
    )
 {% endmacro %}

{% macro create_empty_table_from_relation(relation, source_relation, sql=none) -%}
  {%- set sql_header = config.get('sql_header', none) -%}
  {%- if sql -%}
    {%- set columns = adapter.get_column_schema_from_query(sql) | list -%}
  {%- else -%}
    {%- set columns = adapter.get_columns_in_relation(source_relation) | list -%}
  {%- endif -%}
  {%- set col_list = [] -%}
  {% for col in columns %}
    {{col_list.append(col.name + ' ' + col.data_type) or '' }}
  {% endfor %}
  {{ sql_header if sql_header is not none }}

  create table {{ relation.include(database=False) }}
  {{ on_cluster_clause(relation) }} (
      {{col_list | join(', ')}}

    {% if config.get('projections') %}
      {% set projections = config.get('projections') %}
      {% for projection in projections %}
        , PROJECTION {{ projection.get("name") }} (
            {{ projection.get("query") }}
        )
      {% endfor %}
  {% endif %}
  )

  {{ engine_clause() }}
  {{ order_cols(label="order by") }}
  {{ primary_key_clause(label="primary key") }}
  {{ partition_cols(label="partition by") }}
  {{ ttl_config(label="ttl")}}
  {{ adapter.get_model_settings(model, config.get('engine', default='MergeTree')) }}
{%- endmacro %}

{% macro create_distributed_local_table(distributed_relation, shard_relation, structure_relation, sql_query=none, has_contract=false) -%}
  {{ drop_relation_if_exists(shard_relation) }}
  {{ drop_relation_if_exists(distributed_relation) }}
  {{ create_schema(shard_relation) }}
  {% do run_query(create_empty_table_from_relation(shard_relation, structure_relation, sql_query)) or '' %}
  {% do run_query(create_distributed_table(distributed_relation, shard_relation)) or '' %}
  {% if sql_query is not none %}
    {% do run_query(clickhouse__insert_into(distributed_relation, sql_query, has_contract)) or '' %}
  {% endif %}
{%- endmacro %}
