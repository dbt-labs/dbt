{%- materialization view, adapter='salesforce', supported_languages=['sql'] %}
{{ exceptions.raise_not_implemented(
  'view materialization not implemented for adapter '+adapter.type()) }}
{% endmaterialization %}
