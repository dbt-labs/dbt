{% materialization table, adapter='snowflake', supported_languages=['sql', 'python']%}

  {% set original_query_tag = set_query_tag() %}

  {%- set identifier = model['alias'] -%}
  {%- set language = model['language'] -%}

  {% set grant_config = config.get('grants') %}

  {%- set existing_relation = adapter.get_relation(database=database, schema=schema, identifier=identifier) -%}

  {%- set catalog_relation = adapter.build_catalog_relation(config.model) -%}
  {%- set target_relation = api.Relation.create(
	identifier=identifier,
	schema=schema,
	database=database,
	type='table',
	table_format=catalog_relation.table_format
   ) -%}

  {{ run_hooks(pre_hooks) }}

  {% if target_relation.needs_to_drop(existing_relation) %}
    {{ drop_relation_if_exists(existing_relation) }}
  {% endif %}

  {% call statement('main', language=language) -%}
      {{ create_table_as(False, target_relation, compiled_code, language) }}
  {%- endcall %}

  {{ run_hooks(post_hooks) }}

  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode=True) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  {% do unset_query_tag(original_query_tag) %}

  {{ return({'relations': [target_relation]}) }}

{% endmaterialization %}
