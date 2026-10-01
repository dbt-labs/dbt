{% macro salesforce__scalar_function_sql(target_relation) %}
{{ exceptions.raise_not_implemented(
  'scalar_function_sql macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

{% macro salesforce__scalar_function_create_replace_signature_sql(target_relation) %}
{{ exceptions.raise_not_implemented(
  'scalar_function_create_replace_signature_sql macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

{% macro salesforce__formatted_scalar_function_args_sql() %}
{{ exceptions.raise_not_implemented(
  'formatted_scalar_function_args_sql macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

{% macro salesforce__formatted_scalar_function_args_javascript() %}
{{ exceptions.raise_not_implemented(
  'formatted_scalar_function_args_javascript macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

{% macro salesforce__scalar_function_body_sql() %}
{{ exceptions.raise_not_implemented(
  'scalar_function_body_sql macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

{% macro salesforce__scalar_function_volatility_javascript() %}
{{ exceptions.raise_not_implemented(
  'scalar_function_volatility_javascript macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

{% macro salesforce__scalar_function_volatility_sql() %}
{{ exceptions.raise_not_implemented(
  'scalar_function_volatility_sql macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

{% macro salesforce__unsupported_volatility_warning(volatility) %}
{{ exceptions.raise_not_implemented(
  'unsupported_volatility_warning macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}
