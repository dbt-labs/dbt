{% macro athena__datediff(first_date, second_date, datepart) -%}
    {%- if datepart == 'year' -%}
        (year(CAST({{ second_date }} AS TIMESTAMP)) - year(CAST({{ first_date }} AS TIMESTAMP)))
    {%- elif datepart == 'quarter' -%}
        ({{ datediff(first_date, second_date, 'year') }} * 4) + quarter(CAST({{ second_date }} AS TIMESTAMP)) - quarter(CAST({{ first_date }} AS TIMESTAMP))
    {%- elif datepart == 'month' -%}
        ({{ datediff(first_date, second_date, 'year') }} * 12) + month(CAST({{ second_date }} AS TIMESTAMP)) - month(CAST({{ first_date }} AS TIMESTAMP))
    {%- elif datepart == 'day' -%}
        ((to_milliseconds((CAST(CAST({{ second_date }} AS TIMESTAMP) AS DATE) - CAST(CAST({{ first_date }} AS TIMESTAMP) AS DATE)))) / 86400000)
    {%- elif datepart == 'week' -%}
         ({{ datediff(first_date, second_date, 'day') }} / 7 + case
            when dow(CAST({{first_date}} AS TIMESTAMP)) <= dow(CAST({{second_date}} AS TIMESTAMP)) then
                case when {{first_date}} <= {{second_date}} then 0 else -1 end
            else
                case when {{first_date}} <= {{second_date}} then 1 else 0 end
        end)
    {%- elif datepart == 'hour' -%}
        ({{ datediff(first_date, second_date, 'day') }} * 24 + hour(CAST({{ second_date }} AS TIMESTAMP)) - hour(CAST({{ first_date }} AS TIMESTAMP)))
    {%- elif datepart == 'minute' -%}
        ({{ datediff(first_date, second_date, 'hour') }} * 60 + minute(CAST({{ second_date }} AS TIMESTAMP)) - minute(CAST({{ first_date }} AS TIMESTAMP)))
    {%- elif datepart == 'second' -%}
        ({{ datediff(first_date, second_date, 'minute') }} * 60 + second(CAST({{ second_date }} AS TIMESTAMP)) - second(CAST({{ first_date }} AS TIMESTAMP)))
    {%- elif datepart == 'millisecond' -%}
        (to_milliseconds((CAST({{ second_date }} AS TIMESTAMP) - CAST({{ first_date }} AS TIMESTAMP))))
    {%- else -%}
        {% if execute %}{{ exceptions.raise_compiler_error("Unsupported datepart for macro datediff in Athena: {!r}".format(datepart)) }}{% endif %}
    {%- endif -%}
{%- endmacro %}
