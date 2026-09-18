{% macro athena__listagg(measure, delimiter_text, order_by_clause, limit_num) -%}
    array_join(
        {%- if limit_num %}
          slice(
        {%- endif %}
          array_agg(
              {{ measure }}
              {%- if order_by_clause %}
                {{ order_by_clause }}
              {%- endif %}
          )
        {%- if limit_num %}
            , 1, {{ limit_num }}
          )
        {%- endif %}
        , {{ delimiter_text }}
    )
{%- endmacro %}
