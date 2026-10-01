-- funcsign: () -> string
{% macro salesforce__current_timestamp() -%}
CURRENT_TIMESTAMP
{%- endmacro %}

{#
-- funcsign: () -> string
{% macro salesforce__current_timestamp_backcompat() -%}
{{ exceptions.raise_not_implemented('current_timestamp_backcompat macro not implemented for adapter salesforce') }}
{% endmacro %}
#}

{#
-- funcsign: () -> string
{% macro salesforce__current_timestamp_in_utc_backcompat() -%}
{{ exceptions.raise_not_implemented('current_timestamp_in_utc_backcompat macro not implemented for adapter salesforce') }}
{% endmacro %}
#}
