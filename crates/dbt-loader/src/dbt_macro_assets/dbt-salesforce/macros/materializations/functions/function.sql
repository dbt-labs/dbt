{% materialization function, adapter='salesforce', supported_languages=['sql', 'python', 'javascript'] %}
{{ exceptions.raise_not_implemented(
  'function materialization not implemented for adapter '+adapter.type()) }}
{% endmaterialization %}
