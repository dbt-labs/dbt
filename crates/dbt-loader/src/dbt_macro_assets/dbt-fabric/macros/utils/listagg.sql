{% macro fabric__listagg(measure, delimiter_text, order_by_clause, limit_num) -%}

    string_agg({{ measure }}, {{ delimiter_text }})
        {%- if order_by_clause != None %}
            within group ({{ order_by_clause }})
        {%- endif %}

{%- endmacro %}
