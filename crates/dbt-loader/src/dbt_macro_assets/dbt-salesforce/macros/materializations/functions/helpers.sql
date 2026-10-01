{% macro salesforce__function_execute_build_sql(build_sql, existing_relation, target_relation) %}
{{ exceptions.raise_not_implemented(
  'function_execute_build_sql macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

{% macro salesforce__get_function_macro(function_type, function_language) %}
{{ exceptions.raise_not_implemented(
  'get_function_macro macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}
