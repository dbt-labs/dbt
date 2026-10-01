{% macro postgres__datediff(first_date, second_date, datepart) -%}

    {% if datepart == 'year' %}
        (date_part('year', ({{second_date}})::date) - date_part('year', ({{first_date}})::date))
    {% elif datepart == 'quarter' %}
        ({{ datediff(first_date, second_date, 'year') }} * 4 + date_part('quarter', ({{second_date}})::date) - date_part('quarter', ({{first_date}})::date))
    {% elif datepart == 'month' %}
        ({{ datediff(first_date, second_date, 'year') }} * 12 + date_part('month', ({{second_date}})::date) - date_part('month', ({{first_date}})::date))
    {% elif datepart == 'day' %}
        (({{second_date}})::date - ({{first_date}})::date)
    {% elif datepart == 'week' %}
        ({{ datediff(first_date, second_date, 'day') }} / 7 + case
            when date_part('dow', ({{first_date}})::timestamp) <= date_part('dow', ({{second_date}})::timestamp) then
                case when {{first_date}} <= {{second_date}} then 0 else -1 end
            else
                case when {{first_date}} <= {{second_date}} then 1 else 0 end
        end)
    {% elif datepart == 'hour' %}
        ({{ datediff(first_date, second_date, 'day') }} * 24 + date_part('hour', ({{second_date}})::timestamp) - date_part('hour', ({{first_date}})::timestamp))
    {% elif datepart == 'minute' %}
        ({{ datediff(first_date, second_date, 'hour') }} * 60 + date_part('minute', ({{second_date}})::timestamp) - date_part('minute', ({{first_date}})::timestamp))
    {% elif datepart == 'second' %}
        ({{ datediff(first_date, second_date, 'minute') }} * 60 + floor(date_part('second', ({{second_date}})::timestamp)) - floor(date_part('second', ({{first_date}})::timestamp)))
    {% elif datepart == 'millisecond' %}
        ({{ datediff(first_date, second_date, 'minute') }} * 60000 + floor(date_part('millisecond', ({{second_date}})::timestamp)) - floor(date_part('millisecond', ({{first_date}})::timestamp)))
    {% elif datepart == 'microsecond' %}
        ({{ datediff(first_date, second_date, 'minute') }} * 60000000 + floor(date_part('microsecond', ({{second_date}})::timestamp)) - floor(date_part('microsecond', ({{first_date}})::timestamp)))
    {% else %}
        {{ exceptions.raise_compiler_error("Unsupported datepart for macro datediff in postgres: {!r}".format(datepart)) }}
    {% endif %}

{%- endmacro %}
