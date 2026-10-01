{%- materialization dictionary, adapter='clickhouse' -%}

  {%- set existing_relation = load_cached_relation(this) -%}
  {%- set target_relation = this.incorporate(type='dictionary') -%}
  {%- set cluster_clause = on_cluster_clause(target_relation) -%}

  {%- set grant_config = config.get('grants') -%}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}


  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  {# create our new dictionary #}
  {% call statement('main') -%}
    {{ clickhouse__get_create_dictionary_as_sql(target_relation, cluster_clause, sql) }}
  {%- endcall %}

  {% set should_revoke = should_revoke(target_relation, full_refresh_mode=True) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  {{ run_hooks(post_hooks, inside_transaction=True) }}

  {{ adapter.commit() }}


  {{ run_hooks(post_hooks, inside_transaction=False) }}

  {{ return({'relations': [target_relation]}) }}

{%- endmaterialization -%}


{% macro clickhouse__get_create_dictionary_as_sql(relation, cluster_clause, sql) %}
  {%- set fields = config.get('fields') -%}
  {%- set source_type = config.get('source_type') -%}

  CREATE OR REPLACE DICTIONARY {{ relation }} {{ cluster_clause }}
  (
  {%- for (name, data_type) in fields -%}
    {{ name }} {{ data_type }}{%- if not loop.last -%},{%- endif -%}
  {%- endfor -%}
  )
  {{ primary_key_clause(label="primary key") }}
  SOURCE(
    {%- if source_type == 'http' %}
      {{ http_source() }}
    {% else %}
      {{ clickhouse_source(sql) }}
    {% endif -%}
  )
  LAYOUT({{ config.get('layout') }})
  {%- if config.get('lifetime') %}
  LIFETIME({{ config.get('lifetime') }})
  {%- endif %}
  {%- if config.get('range') %}
  RANGE({{ config.get('range') }})
  {%- endif %}
{% endmacro %}


{% macro http_source() %}
  HTTP(URL '{{ config.get("url") }}' FORMAT '{{ config.get("format") }}')
{% endmacro %}


{% macro clickhouse_source(sql) %}
  {%- set credentials = adapter.get_credentials(config.get("connection_overrides", {})) -%}
  {%- set table = config.get('table') -%}
  CLICKHOUSE(
      {% if credentials.get("user") -%}
      user '{{ credentials.get("user") }}'
      {%- endif %}
      {% if credentials.get("password") -%}
      password '{{ credentials.get("password") }}'
      {%- endif %}
      {% if credentials.get("database") -%}
      db '{{ credentials.get("database") }}'
      {%- endif %}
      {%- if table is not none %}
        table '{{ table }}'
      {% else %}
        query "{{ sql }}"
      {% endif -%}
      {%- if config.get('update_field') %}
      update_field '{{ config.get('update_field') }}'
      {%- endif %}
      {%- if config.get('update_lag') is not none %}
      update_lag {{ config.get('update_lag') }}
      {%- endif %}
  )
{% endmacro %}


{% macro drop_dictionary_if_exists(relation, cluster_clause) %}
  {% if relation.type != 'dictionary' %}
    {{ log(relation ~ ' is not a dictionary; defaulting to drop_relation_if_exists') }}
    {{ drop_relation_if_exists(relation) }}
  {% else %}
    {% call statement('drop_dictionary_if_exists') %}
      drop dictionary if exists {{ relation }} {{ cluster_clause }}
    {% endcall %}
  {% endif %}
{% endmacro %}
