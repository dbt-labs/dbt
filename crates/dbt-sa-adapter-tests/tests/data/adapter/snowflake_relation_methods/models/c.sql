{% set table_name = 'a' %}
{% set db_schema = t_schema_name() %}
{% set db_name = target.database %}

{% do set_up(db_schema, table_name) %}

{{ print(db_name) }}
{{ print(db_schema) }}

{% set relation = adapter.get_relation(
      database=db_name,
      schema=db_schema,
      identifier='a')
%}
{{ print('relation: ' ~ relation) }}


{# FIXME: In real use cases, this should be config.model.config, but since RuntimeConfig is not supported properly, we use a hardcoded config as a workaround #}
{% set relation_config = {
      'base_location_root': '_dbt',
      'base_location_subpath': 'test',
      'external_volume': 'test',
} %}
{% set result = relation.get_iceberg_ddl_options(relation_config) %}
{{ print('get_iceberg_ddl_options: ' ~ result)}}

SELECT 1 as one
