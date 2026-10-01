{% macro duckdb__datediff(first_date, second_date, datepart) -%}
    {% if datepart == 'week' %}
            ({{ datediff(first_date, second_date, 'day') }} // 7 + case
            when date_part('dow', ({{first_date}})::timestamp) <= date_part('dow', ({{second_date}})::timestamp) then
                case when {{first_date}} <= {{second_date}} then 0 else -1 end
            else
                case when {{first_date}} <= {{second_date}} then 1 else 0 end
        end)
    {% else %}
        (date_diff('{{ datepart }}', {{ first_date }}::timestamp, {{ second_date}}::timestamp ))
    {% endif %}
{%- endmacro %}
