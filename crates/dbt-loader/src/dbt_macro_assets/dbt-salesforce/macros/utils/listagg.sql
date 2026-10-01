{% macro salesforce__listagg(measure, delimiter_text, order_by_clause, limit_num) -%}
{# TODO:
    Salesforce has `array_to_string`, but not the rest.

    `array_slice` is not a function,
    but instead uses a python-like `(<array>)[<start>:<end>]` syntax
    to slice from start to end, inclusive. Both start and end are 1-based index.

    `array_agg` and `listagg` don't appear to have an equivalent
#}

{{ exceptions.raise_not_implemented('listagg macro not implemented for adapter salesforce') }}

{#
    {% if limit_num -%}
    array_to_string(
        array_slice(
            array_agg(
                {{ measure }}
            ){% if order_by_clause -%}
            within group ({{ order_by_clause }})
            {%- endif %}
            ,0
            ,{{ limit_num }}
        ),
        {{ delimiter_text }}
        )
    {%- else %}
    listagg(
        {{ measure }},
        {{ delimiter_text }}
        )
        {% if order_by_clause -%}
        within group ({{ order_by_clause }})
        {%- endif %}
    {%- endif %}
#}
{%- endmacro %}
