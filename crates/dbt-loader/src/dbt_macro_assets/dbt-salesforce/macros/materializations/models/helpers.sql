-- funcsign: () -> string
{% macro salesforce__get_primary_key() %}
{% set pk = config.get('primary_key', default = []) %}

{% if pk | length is gt 1 %}

{% set message %}
Salesforce Data 360 requires a single primary key column.
Multiple primary key columns specified for model '{{ model.name }}'.
{% endset %}

{% do exceptions.raise_compiler_error(message) %}
{% elif pk | length is lt 1 %}

{#
{% set message %}
Salesforce Data 360 requires a single primary key column.
No primary key column specified for model '{{ model.name }}'.
{% endset %}

{% do exceptions.raise_compiler_error(message) %}
#}

{{ return('') }}

{% else %}

{{ return(pk[0]) }}

{% endif %}
{% endmacro %}

-- funcsign: (optional[string]) -> string
{% macro salesforce__get_category(fallback = none) %}
{% set allowed_categories = ["Profile", "Engagement", "Other"] %}
{% set fallback_category = fallback or allowed_categories[0] %}
{% set category = config.get('category', default = fallback_category) | title %}
{% if category not in allowed_categories %}
{% set message %}
Invalid category '{{ category }}' specified for model '{{ model.name }}'.
Allowed categories are: {{ allowed_categories | join(", ") }}.
{% endset %}
{% do exceptions.raise_compiler_error(message) %}
{% endif %}
{{ return(category) }}
{% endmacro %}

-- funcsign: () -> string
{% macro salesforce__get_write_mode() %}
{# TODO: handle full-refresh scenario? #}
{% set strategy_to_write_mode = {
"append": "APPEND",
"merge": "MERGE"
} %}
{% set strategy = config.get('incremental_strategy', default = "merge") %}
{% set write_mode = strategy_to_write_mode.get(strategy) %}
{% if write_mode is not string %}
{% set message %}
Invalid incremental strategy '{{ strategy }}' specified for model '{{ model.name }}'.
Allowed incremental strategies are: {{ strategy_to_write_mode.keys() | join(", ") }}.
{% endset %}
{% do exceptions.raise_compiler_error(message) %}
{% endif %}
{{ return(write_mode) }}
{% endmacro -%}
