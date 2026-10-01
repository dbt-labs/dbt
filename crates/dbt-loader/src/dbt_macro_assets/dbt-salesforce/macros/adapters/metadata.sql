{# TODO: verify defaults or create overrides for metadata macros #}

-- funcsign: (relation, list[relation]) -> list[relation]
{% macro salesforce__get_catalog_relations(information_schema, relations) -%}
{{ exceptions.raise_not_implemented(
  'get_catalog_relations macro not implemented for adapter '+adapter.type()) }}
{%- endmacro %}

-- funcsign: (relation, list[string]) -> agate_table
{% macro salesforce__get_catalog(information_schema, schemas) -%}
{{ exceptions.raise_not_implemented(
  'get_catalog macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

-- funcsign: (string) -> string
{% macro salesforce__information_schema_name(database) -%}
{{ exceptions.raise_not_implemented(
  'information_schema_name macro not implemented for adapter '+adapter.type()) }}
{%- endmacro %}

-- funcsign: (string) -> agate_table
{% macro salesforce__list_schemas(database) -%}
{{ exceptions.raise_not_implemented(
  'list_schemas macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

-- funcsign: (information_schema, string) -> agate_table
{% macro salesforce__check_schema_exists(information_schema, schema) -%}
{{ exceptions.raise_not_implemented(
  'check_schema_exists macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

-- funcsign: (relation) -> list[relation]
{% macro salesforce__list_relations_without_caching(schema_relation) %}
  {{ exceptions.raise_not_implemented(
    'list_relations_without_caching macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

-- funcsign: (relation) -> agate_table
{% macro salesforce__get_catalog_for_single_relation(relation) %}
  {{ exceptions.raise_not_implemented(
    'get_catalog_for_single_relation macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

-- funcsign: () -> list[relation]
{% macro salesforce__get_relations() %}
  {{ exceptions.raise_not_implemented(
    'get_relations macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

-- funcsign: (information_schema, list[relation]) -> agate_table
{% macro salesforce__get_relation_last_modified(information_schema, relations) %}
  {{ exceptions.raise_not_implemented(
    'get_relation_last_modified macro not implemented for adapter ' + adapter.type()) }}
{% endmacro %}
