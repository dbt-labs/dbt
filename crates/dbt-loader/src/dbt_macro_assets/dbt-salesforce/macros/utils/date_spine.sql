{% macro salesforce__get_intervals_between(start_date, end_date, datepart) %}
{# TODO: maybe can use `justify_interval` or similar? #}
{{ exceptions.raise_not_implemented('get_intervals_between macro not implemented for adapter salesforce') }}
{% endmacro %}

{% macro salesforce__date_spine(datepart, start_date, end_date) %}
{# TODO: probably a job for the `generate_series` function #}
{{ exceptions.raise_not_implemented('date_spine macro not implemented for adapter salesforce') }}
{% endmacro %}
