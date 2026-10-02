{% macro singlestore__dateadd(datepart, interval, from_date_or_timestamp) %}
    date_add(
        {{ from_date_or_timestamp }},
        INTERVAL {{ interval }} {{ datepart }}
    )
{% endmacro %}

{% macro singlestore__datediff(first_date, second_date, datepart) -%}
    timestampdiff(
        {{ datepart }},
        {{ first_date }},
        {{ second_date }}
    )
{%- endmacro %}

{% macro singlestore__split_part(string_text, delimiter_text, part_number) %}
    if(
        length(split(
            {{ string_text }},
            {{ delimiter_text }}
            ) :> ARRAY(TEXT)) >= {{ part_number }},
        (split(
            {{ string_text }},
            {{ delimiter_text }}
            ) :> ARRAY(TEXT))[{{ part_number - 1 }}],
        NULL)
{% endmacro %}

{% macro singlestore__safe_cast(field, type) %}
    ({{ field }} !:> {{ type }})
{% endmacro %}

{% macro singlestore__listagg(measure, delimiter_text, order_by_clause, limit_num) -%}
    {% if limit_num -%}
    substring_index(
        group_concat(
            {{ measure }}
            {%- if order_by_clause %}
                {{ order_by_clause }}
            {%- endif %}
            SEPARATOR {{ delimiter_text }}
        ),
        {{ delimiter_text }},
        {{ limit_num }}
    )
    {%- else %}
    group_concat(
        {{ measure }}
        {%- if order_by_clause %}
            {{ order_by_clause }}
        {%- endif %}
        SEPARATOR {{ delimiter_text }}
    )
    {%- endif %}
{%- endmacro %}

{% macro singlestore__hash(field) -%}
    md5({{ field }} :> TEXT)
{%- endmacro %}

{% macro singlestore__cast_bool_to_text(field) %}
    if(isnull({{ field }}), null, if({{ field }}, "true", "false"))
{% endmacro %}

{% macro singlestore__bool_or(expression) -%}
    sum({{ expression }})
{%- endmacro %}
