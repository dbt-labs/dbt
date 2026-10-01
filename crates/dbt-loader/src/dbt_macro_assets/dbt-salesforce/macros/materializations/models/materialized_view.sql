{%- materialization materialized_view, adapter='salesforce', supported_languages=['sql'] %}
{{ exceptions.raise_not_implemented(
  'materialized_view materialization not implemented for adapter '+adapter.type()) }}
{% endmaterialization %}
