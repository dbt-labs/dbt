{% macro duckdb__listagg(measure, delimiter_text, order_by_clause, limit_num) -%}
    {% if limit_num -%}
    list_aggr(
        (array_agg(
            {{ measure }}
            {% if order_by_clause -%}
            {{ order_by_clause }}
            {%- endif %}
        ))[1:{{ limit_num }}],
        'string_agg',
        {{ delimiter_text }}
        )
    {%- else %}
    string_agg(
        {{ measure }},
        {{ delimiter_text }}
        {% if order_by_clause -%}
        {{ order_by_clause }}
        {%- endif %}
        )
    {%- endif %}
{%- endmacro %}
