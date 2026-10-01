{% macro bigquery__listagg(measure, delimiter_text, order_by_clause, limit_num) -%}

    string_agg(
        {{ measure }},
        {{ delimiter_text }}
        {% if order_by_clause -%}
        {{ order_by_clause }}
        {%- endif %}
        {% if limit_num -%}
        limit {{ limit_num }}
        {%- endif %}
        )

{%- endmacro %}
