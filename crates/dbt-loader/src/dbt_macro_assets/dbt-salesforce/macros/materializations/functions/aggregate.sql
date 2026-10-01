{% macro salesforce__get_aggregate_function_create_replace_signature(target_relation) %}
{{ exceptions.raise_not_implemented(
  'get_aggregate_function_create_replace_signature macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

{% macro salesforce__get_formatted_aggregate_function_args() %}
{{ exceptions.raise_not_implemented(
  'get_formatted_aggregate_function_args macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

{% macro salesforce__get_function_language_specifier() %}
{{ exceptions.raise_not_implemented(
  'get_function_language_specifier macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

{% macro salesforce__get_aggregate_function_volatility_specifier() %}
{{ exceptions.raise_not_implemented(
  'get_aggregate_function_volatility_specifier macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

{% macro salesforce__get_function_python_options() %}
{{ exceptions.raise_not_implemented(
  'get_function_python_options macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}
